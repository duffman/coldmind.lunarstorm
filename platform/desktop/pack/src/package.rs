//! Release packaging (spec 6.3): archive -> attach to the prebuilt release runtime ->
//! package and sign per OS.
//!
//! | target  | layout                                                         | signing            |
//! |---------|----------------------------------------------------------------|--------------------|
//! | macOS   | `<Name>.app` bundle, archive in `Contents/Resources/app.zebra` | `codesign`         |
//! | Windows | `<name>.exe` with the archive as an overlay                    | `signtool`, if set |
//! | Linux   | `<name>/<name>` + `<name>.zebra` + `<name>.desktop`             | none               |

use lunarstorm_platform::assets::{append_overlay, write_archive};
use lunarstorm_platform::manifest::HostConfig;
use lunarstorm_platform::pipeline::BuildOutput;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Packaging target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// macOS app bundle.
    MacOs,
    /// Windows executable.
    Windows,
    /// Linux directory.
    Linux,
}

impl Target {
    /// The target of the machine running the packager.
    ///
    /// # Returns
    /// The host target.
    pub fn host() -> Target {
        if cfg!(target_os = "macos") {
            Target::MacOs
        } else if cfg!(windows) {
            Target::Windows
        } else {
            Target::Linux
        }
    }
}

/// File-system-safe executable name from the application name.
///
/// # Arguments
/// * `name` - application name.
///
/// # Returns
/// Lowercase, hyphen-separated ASCII; `app` when nothing is left.
pub fn exe_name(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-').to_owned();
    if out.is_empty() { "app".into() } else { out }
}

/// Packages a build with a prebuilt release runtime.
///
/// # Arguments
/// * `build` - pipeline output.
/// * `runtime` - path of the prebuilt `lunarstorm-release` executable for `target`.
/// * `out` - output directory.
/// * `target` - packaging target.
/// * `sign` - signing identity (`-` for ad-hoc on macOS); `None` skips signing except
///   the ad-hoc macOS signature, which macOS requires to run on Apple silicon.
///
/// # Returns
/// The path of the packaged application.
///
/// # Errors
/// I/O failure, or the signing tool failed.
pub fn package(build: &BuildOutput, runtime: &Path, out: &Path, target: Target, sign: Option<&str>) -> Result<PathBuf, String> {
    let io = |what: &Path, e: std::io::Error| format!("{}: {e}", what.display());
    let archive = write_archive(build);
    let app = &build.config.application;
    let exe = exe_name(&app.name);
    std::fs::create_dir_all(out).map_err(|e| io(out, e))?;
    match target {
        Target::MacOs => {
            let bundle = out.join(format!("{}.app", app.name));
            if bundle.exists() {
                std::fs::remove_dir_all(&bundle).map_err(|e| io(&bundle, e))?;
            }
            let (macos, resources) = (bundle.join("Contents/MacOS"), bundle.join("Contents/Resources"));
            std::fs::create_dir_all(&macos).map_err(|e| io(&macos, e))?;
            std::fs::create_dir_all(&resources).map_err(|e| io(&resources, e))?;
            std::fs::copy(runtime, macos.join(&exe)).map_err(|e| io(runtime, e))?;
            std::fs::write(resources.join("app.zebra"), &archive).map_err(|e| io(&resources, e))?;
            let icon = app.icon.as_deref().and_then(|p| build.files.get(p.trim_start_matches('/'))).and_then(|png| icns(png));
            if let Some(icns) = &icon {
                std::fs::write(resources.join("icon.icns"), icns).map_err(|e| io(&resources, e))?;
            }
            let plist = bundle.join("Contents/Info.plist");
            std::fs::write(&plist, info_plist(&build.config, &exe, icon.is_some())).map_err(|e| io(&plist, e))?;
            if cfg!(target_os = "macos") {
                run(Command::new("codesign").args(["--force", "--deep", "--options", "runtime", "--sign", sign.unwrap_or("-")]).arg(&bundle))?;
            }
            Ok(bundle)
        }
        Target::Windows => {
            let bytes = std::fs::read(runtime).map_err(|e| io(runtime, e))?;
            let path = out.join(format!("{exe}.exe"));
            // The overlay goes on before signing: signtool appends the certificate after it.
            std::fs::write(&path, append_overlay(&bytes, &archive)).map_err(|e| io(&path, e))?;
            if let Some(identity) = sign {
                run(Command::new("signtool").args(["sign", "/fd", "SHA256", "/n", identity]).arg(&path))?;
            }
            Ok(path)
        }
        Target::Linux => {
            let dir = out.join(&exe);
            std::fs::create_dir_all(&dir).map_err(|e| io(&dir, e))?;
            let bin = dir.join(&exe);
            std::fs::copy(runtime, &bin).map_err(|e| io(runtime, e))?;
            std::fs::write(bin.with_extension("zebra"), &archive).map_err(|e| io(&dir, e))?;
            let desktop = format!(
                "[Desktop Entry]\nType=Application\nName={}\nExec={exe} %U\nTerminal=false\n{}",
                app.name,
                if app.url_schemes.is_empty() {
                    String::new()
                } else {
                    format!("MimeType={}\n", app.url_schemes.iter().map(|s| format!("x-scheme-handler/{s};")).collect::<String>())
                }
            );
            std::fs::write(dir.join(format!("{exe}.desktop")), desktop).map_err(|e| io(&dir, e))?;
            Ok(dir)
        }
    }
}

/// Runs a tool and fails with its output when it fails.
///
/// # Arguments
/// * `command` - the command.
///
/// # Returns
/// Nothing.
///
/// # Errors
/// The tool is missing or exited non-zero.
fn run(command: &mut Command) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command.output().map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed: {}", String::from_utf8_lossy(&output.stderr)))
    }
}

/// Escapes text for a plist string.
///
/// # Arguments
/// * `text` - raw text.
///
/// # Returns
/// XML-escaped text.
fn xml(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// Writes `Info.plist` from the `<application>` element and `<document>` elements.
///
/// # Arguments
/// * `config` - host configuration.
/// * `exe` - executable name.
/// * `icon` - whether `icon.icns` was written.
///
/// # Returns
/// The plist XML.
pub fn info_plist(config: &HostConfig, exe: &str, icon: bool) -> String {
    let app = &config.application;
    let mut keys = vec![
        ("CFBundleIdentifier", format!("<string>{}</string>", xml(&app.id))),
        ("CFBundleName", format!("<string>{}</string>", xml(&app.name))),
        ("CFBundleDisplayName", format!("<string>{}</string>", xml(&app.name))),
        ("CFBundleExecutable", format!("<string>{}</string>", xml(exe))),
        ("CFBundlePackageType", "<string>APPL</string>".into()),
        ("CFBundleShortVersionString", format!("<string>{}</string>", xml(&app.version))),
        ("CFBundleVersion", format!("<string>{}</string>", xml(&app.version))),
        ("NSHighResolutionCapable", "<true/>".into()),
        ("LSMinimumSystemVersion", "<string>11.0</string>".into()),
    ];
    if icon {
        keys.push(("CFBundleIconFile", "<string>icon</string>".into()));
    }
    if app.activation_policy.as_deref() == Some("accessory") {
        keys.push(("LSUIElement", "<true/>".into()));
    }
    if !app.url_schemes.is_empty() {
        let schemes: String = app.url_schemes.iter().map(|s| format!("<string>{}</string>", xml(s))).collect();
        keys.push((
            "CFBundleURLTypes",
            format!("<array><dict><key>CFBundleURLName</key><string>{}</string><key>CFBundleURLSchemes</key><array>{schemes}</array></dict></array>", xml(&app.id)),
        ));
    }
    let documents: String = config
        .documents
        .iter()
        .filter_map(|d| {
            let name = d.get("name").and_then(|v| v.as_str())?;
            let exts: String = d
                .get("extensions")
                .and_then(|v| v.as_array())
                .into_iter()
                .flatten()
                .filter_map(|e| e.as_str())
                .map(|e| format!("<string>{}</string>", xml(e.trim_start_matches('.'))))
                .collect();
            Some(format!(
                "<dict><key>CFBundleTypeName</key><string>{}</string><key>CFBundleTypeRole</key><string>Editor</string><key>CFBundleTypeExtensions</key><array>{exts}</array></dict>",
                xml(name)
            ))
        })
        .collect();
    if !documents.is_empty() {
        keys.push(("CFBundleDocumentTypes", format!("<array>{documents}</array>")));
    }
    let body: String = keys.into_iter().map(|(k, v)| format!("  <key>{k}</key>{v}\n")).collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n{body}</dict>\n</plist>\n"
    )
}

/// Wraps a PNG in an `.icns` file (PNG-compressed entries, macOS 10.7+), resizing to the
/// nearest supported size.
///
/// # Arguments
/// * `png` - PNG bytes.
///
/// # Returns
/// The icns bytes, or `None` when the image does not decode.
pub fn icns(png: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory(png).ok()?;
    let side = image.width().min(image.height());
    let (kind, size) = [(b"ic10", 1024u32), (b"ic09", 512), (b"ic08", 256), (b"ic07", 128)]
        .into_iter()
        .find(|(_, s)| side >= *s)
        .unwrap_or((b"ic07", 128));
    let resized = image.resize_exact(size, size, image::imageops::FilterType::Lanczos3);
    let mut encoded = Vec::new();
    resized.write_to(&mut std::io::Cursor::new(&mut encoded), image::ImageFormat::Png).ok()?;
    let mut out = Vec::with_capacity(encoded.len() + 16);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&((encoded.len() + 16) as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(&((encoded.len() + 8) as u32).to_be_bytes());
    out.extend_from_slice(&encoded);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunarstorm_platform::assets::{attached_path, read_archive, split_overlay};
    use lunarstorm_platform::pipeline::{OutputDirPipeline, Pipeline};

    /// The example app in `platform/desktop/examples/app`, built through the real pipeline.
    fn sample_build() -> BuildOutput {
        OutputDirPipeline::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../examples/app")).build().expect("example builds")
    }

    /// A scratch directory under the system temp dir.
    ///
    /// # Arguments
    /// * `tag` - test tag.
    ///
    /// # Returns
    /// A fresh directory.
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lunarstorm-pack-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn names() {
        assert_eq!(exe_name("Project Register"), "project-register");
        assert_eq!(exe_name("  ***  "), "app");
    }

    #[test]
    fn windows_and_linux_layouts_carry_a_readable_archive() {
        let build = sample_build();
        let dir = scratch("wl");
        let runtime = dir.join("lunarstorm-release");
        std::fs::write(&runtime, b"MZ prebuilt runtime").unwrap();

        let exe = package(&build, &runtime, &dir.join("win"), Target::Windows, None).unwrap();
        let bytes = std::fs::read(&exe).unwrap();
        assert!(bytes.starts_with(b"MZ prebuilt runtime"));
        assert_eq!(read_archive(split_overlay(&bytes).unwrap()).unwrap().config, build.config);

        let linux = package(&build, &runtime, &dir.join("linux"), Target::Linux, None).unwrap();
        let bin = linux.join("project-register");
        assert_eq!(read_archive(&std::fs::read(attached_path(&bin)).unwrap()).unwrap().config, build.config);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn plist_has_identity_schemes_and_documents() {
        let mut config = sample_build().config;
        config.application.url_schemes = vec!["register".into()];
        config.documents = vec![serde_json::json!({"name": "Register file", "extensions": [".reg"]})];
        let plist = info_plist(&config, "project-register", false);
        assert!(plist.contains("<key>CFBundleIdentifier</key><string>dev.zebra.example</string>"));
        assert!(plist.contains("<string>register</string>"));
        assert!(plist.contains("<key>CFBundleTypeExtensions</key><array><string>reg</string></array>"));
    }

    #[test]
    fn icns_wraps_a_png() {
        let image = image::RgbaImage::from_pixel(300, 300, image::Rgba([10, 20, 30, 255]));
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        let icns = icns(&png).unwrap();
        assert_eq!(&icns[..4], b"icns");
        assert_eq!(&icns[8..12], b"ic08");
        assert_eq!(u32::from_be_bytes(icns[4..8].try_into().unwrap()) as usize, icns.len());
    }
}
