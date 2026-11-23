//! Dependency-free command parsing and discoverable, command-specific help.
use anyhow::{bail, Context};
use devermind_bundler::BuildOptions;
use devermind_server::Config;
use std::{iter::Peekable, path::PathBuf, vec::IntoIter};

use crate::transpile;

type Args = Peekable<IntoIter<String>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topic {
    Dev,
    Build,
    Preview,
    Transpile,
}

impl Topic {
    fn parse(name: &str) -> Option<Self> {
        match name {
            "dev" => Some(Self::Dev),
            "build" => Some(Self::Build),
            "preview" => Some(Self::Preview),
            "transpile" => Some(Self::Transpile),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Build => "build",
            Self::Preview => "preview",
            Self::Transpile => "transpile",
        }
    }
}

pub enum Command {
    Help(Option<Topic>),
    Version,
    Dev(Config),
    Build(BuildOptions),
    Preview {
        root: PathBuf,
        out: Option<PathBuf>,
        port: u16,
    },
    Transpile(transpile::Options),
}

/// The frontend commands of `jetbunny --help`.
pub fn usage() -> String {
    r#"Frontend development server, production builds and standalone transpilation:
  jetbunny --serve [root]          Serve source modules and watch for hot updates
  jetbunny --build [root]          Bundle a production frontend
  jetbunny --preview [root]        Serve an existing production build locally
  jetbunny --transpile <file|->    Transpile one TS/TSX/JS/JSX module, without bundling
Use `jetbunny <command> --help` for options and examples."#
        .to_string()
}

/// Global help describes each operation; subcommand help exposes only its own flags.
/// No terminal styling is assumed, so output is also useful in redirected logs.
pub fn help(topic: Option<Topic>) -> String {
    let body = match topic {
        None => return usage(),
        Some(Topic::Dev) => {
            r#"Usage: jetbunny --serve [root] [options]

Serve source modules, watch dependencies and send hot updates.

Options:
  --host HOST                 Bind address (default: 127.0.0.1)
  --port PORT                 Development port (default: 5173; 0 selects a free port)
  --define KEY=VALUE          Compile-time JS expression; may be repeated
  --no-watch                  Disable file watching
  -h, --help                  Show this help

Root defaults to the current directory. Explicit options override native config.
Example: jetbunny --serve ./app --port 5173"#
        }
        Some(Topic::Build) => {
            r#"Usage: jetbunny --build [root] [options]

Bundle an HTML frontend into production JavaScript, CSS and assets.

Options:
  -o, --out DIR               Output directory (config/default: dist)
  --no-minify                  Disable production minification
  -h, --help                  Show this help

Root defaults to the current directory. Builds replace the output directory.
Example: jetbunny --build ./app --out dist"#
        }
        Some(Topic::Preview) => {
            r#"Usage: jetbunny --preview [root] [options]

Serve an existing production build on loopback, including SPA routes.

Options:
  -o, --out DIR               Existing output directory (config/default: dist)
  --port PORT                 Preview port (default: 4173; 0 selects a free port)
  -h, --help                  Show this help

Preview does not build first. Root defaults to the current directory.
Example: jetbunny --preview ./app --port 4173"#
        }
        Some(Topic::Transpile) => {
            r#"Usage: jetbunny --transpile <file|-> [options]

Transpile source syntax to JavaScript; preserve module syntax. No bundling or type-checking.

Options:
  -o, --out FILE              Write JavaScript to FILE (default: stdout; '-' also means stdout)
  --sourcemap                 Emit an external map with --out, or inline map on stdout
  --loader ts|tsx|js|jsx       Syntax for stdin input '-' (default: ts; stdin only)
  -h, --help                  Show this help

Imports remain unchanged. No native or Vite project configuration is loaded.
File syntax is inferred from its extension; use -- before dash-prefixed paths.

Examples:
  jetbunny --transpile src/main.ts
  jetbunny --transpile src/App.tsx --out dist/App.js --sourcemap
  cat src/main.ts | jetbunny --transpile - --loader ts"#
        }
    };
    body.to_string()
}

/// Preserves explicit overrides and the existing development-root shorthand.
/// `--` ends option parsing; value options accept both `--flag value` and
/// `--flag=value`. A following flag is never consumed as a missing value.
pub fn parse(args: impl IntoIterator<Item = String>) -> anyhow::Result<Command> {
    let mut args = args.into_iter().collect::<Vec<_>>().into_iter().peekable();
    if args.peek().is_some_and(|arg| arg == "help") {
        args.next();
        let topic = match args.next().as_deref() {
            None | Some("--help" | "-h") => None,
            Some(name) => {
                Some(Topic::parse(name).with_context(|| format!("unknown command '{name}'"))?)
            }
        };
        if let Some(extra) = args.next() {
            bail!("unexpected argument to help: {extra}");
        }
        return Ok(Command::Help(topic));
    }
    let explicit = args.peek().and_then(|arg| Topic::parse(arg));
    let topic = explicit.unwrap_or(Topic::Dev);
    if explicit.is_some() {
        args.next();
    }
    if topic == Topic::Transpile {
        return parse_transpile(args);
    }
    let mut config = Config::new(".");
    let mut root_seen = false;
    let mut out = None;
    let mut minify = None;
    let mut preview_port = 4173;
    let mut positional = false;
    while let Some(arg) = args.next() {
        if positional {
            set_root(&mut config.root, &mut root_seen, arg)?;
            continue;
        }
        let (flag, inline) = option(&arg);
        match flag {
            "--" => positional = true,
            "-h" | "--help" if inline.is_none() => return Ok(Command::Help(explicit)),
            "-V" | "--version" if inline.is_none() => return Ok(Command::Version),
            "--port" if topic != Topic::Build => {
                let value = value(flag, inline, &mut args)?;
                let port = value
                    .parse()
                    .context("--port must be an integer from 0 to 65535")?;
                if topic == Topic::Dev {
                    config.set_port(port);
                } else {
                    preview_port = port;
                }
            }
            "--host" if topic == Topic::Dev => config.set_host(value(flag, inline, &mut args)?),
            "--define" if topic == Topic::Dev => {
                let pair = value(flag, inline, &mut args)?;
                let (key, expression) = pair.split_once('=').context("--define needs KEY=VALUE")?;
                if key.is_empty() || expression.is_empty() {
                    bail!("--define needs a non-empty key and expression");
                }
                config
                    .define
                    .push((key.to_string(), expression.to_string()));
            }
            "--no-watch" if topic == Topic::Dev && inline.is_none() => config.watch = false,
            "-o" | "--out" if topic != Topic::Dev => {
                out = Some(value(flag, inline, &mut args)?.into())
            }
            "--no-minify" if topic == Topic::Build && inline.is_none() => minify = Some(false),
            flag if flag.starts_with('-') => bail!("unsupported {} option {arg}", topic.name()),
            _ => set_root(&mut config.root, &mut root_seen, arg)?,
        }
    }
    Ok(match topic {
        Topic::Build => Command::Build(BuildOptions {
            root: config.root,
            out,
            minify,
            production: true,
        }),
        Topic::Preview => Command::Preview {
            root: config.root,
            out,
            port: preview_port,
        },
        _ => Command::Dev(config),
    })
}

fn parse_transpile(mut args: Args) -> anyhow::Result<Command> {
    let mut input = None;
    let mut out = None;
    let mut sourcemap = false;
    let mut loader = None;
    let mut positional = false;
    while let Some(arg) = args.next() {
        if !positional {
            let (flag, inline) = option(&arg);
            match flag {
                "--" => {
                    positional = true;
                    continue;
                }
                "-h" | "--help" if inline.is_none() => {
                    return Ok(Command::Help(Some(Topic::Transpile)))
                }
                "-V" | "--version" if inline.is_none() => return Ok(Command::Version),
                "-o" | "--out" => {
                    out = Some(value(flag, inline, &mut args)?.into());
                    continue;
                }
                "--sourcemap" if inline.is_none() => {
                    sourcemap = true;
                    continue;
                }
                "--loader" => {
                    let value = value(flag, inline, &mut args)?;
                    if !matches!(value.as_str(), "ts" | "tsx" | "js" | "jsx") {
                        bail!("unsupported loader '{value}'; expected ts, tsx, js or jsx");
                    }
                    loader = Some(value);
                    continue;
                }
                "-" => {}
                flag if flag.starts_with('-') => bail!("unsupported transpile option {arg}"),
                _ => {}
            }
        }
        if input.replace(PathBuf::from(&arg)).is_some() {
            bail!("transpile accepts one input file; unexpected argument {arg}");
        }
    }
    let input = input.context("transpile requires an input file or '-' for stdin")?;
    if loader.is_some() && input != std::path::Path::new("-") {
        bail!(
            "--loader is only supported with stdin input '-'; file syntax comes from its extension"
        );
    }
    Ok(Command::Transpile(transpile::Options {
        input,
        out,
        sourcemap,
        loader,
    }))
}

fn option(arg: &str) -> (&str, Option<&str>) {
    if arg.starts_with('-') {
        if let Some(pair) = arg.split_once('=') {
            return (pair.0, Some(pair.1));
        }
    }
    (arg, None)
}

fn value(flag: &str, inline: Option<&str>, args: &mut Args) -> anyhow::Result<String> {
    let value = if let Some(value) = inline {
        value.to_string()
    } else {
        if args
            .peek()
            .is_some_and(|value| value != "-" && value.starts_with('-'))
        {
            bail!("{flag} needs a value before the next option; use {flag}=VALUE for a value starting with '-'");
        }
        args.next()
            .with_context(|| format!("{flag} needs a value"))?
    };
    if value.is_empty() {
        bail!("{flag} needs a non-empty value");
    }
    Ok(value)
}

fn set_root(root: &mut PathBuf, seen: &mut bool, value: String) -> anyhow::Result<()> {
    if *seen {
        bail!("unexpected extra root {value}");
    }
    *root = value.into();
    *seen = true;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn shorthand_and_dev_preserve_overrides() {
        for values in [
            args(&["app", "--port", "5173"]),
            args(&["dev", "app", "--no-watch"]),
        ] {
            let Command::Dev(config) = parse(values).unwrap() else {
                panic!("expected dev")
            };
            assert_eq!(config.root, PathBuf::from("app"));
        }
    }

    #[test]
    fn build_preserves_optional_overrides() {
        let Command::Build(options) =
            parse(args(&["build", "app", "--out", "dist", "--no-minify"])).unwrap()
        else {
            panic!("expected build")
        };
        assert_eq!(options.out, Some(PathBuf::from("dist")));
        assert_eq!(options.minify, Some(false));
        let Command::Build(options) = parse(args(&["build"])).unwrap() else {
            panic!("expected build")
        };
        assert!(options.out.is_none() && options.minify.is_none());
    }

    #[test]
    fn invalid_flags_and_missing_values_fail() {
        for values in [
            args(&["build", "--host", "localhost"]),
            args(&["dev", "--out", "dist"]),
            args(&["preview", "--port"]),
            args(&["dev", "--define", "=value"]),
            args(&["dev", "one", "two"]),
            args(&["build", "--out", "--no-minify"]),
            args(&["transpile", "--out", "file.js"]),
            args(&["help", "unknown"]),
        ] {
            assert!(parse(values).is_err());
        }
    }

    #[test]
    fn equals_options_and_option_terminator_are_preserved() {
        let Command::Dev(config) = parse(args(&["dev", "--port=0", "--", "-project"])).unwrap()
        else {
            panic!("expected dev")
        };
        assert_eq!(config.root, PathBuf::from("-project"));
        assert_eq!(config.port, 0);
        let Command::Transpile(options) =
            parse(args(&["transpile", "--out=result.js", "--", "-input.ts"])).unwrap()
        else {
            panic!("expected transpile")
        };
        assert_eq!(options.input, PathBuf::from("-input.ts"));
        assert_eq!(options.out, Some("result.js".into()));
    }

    #[test]
    fn stdin_loader_and_command_help_are_explicit() {
        let Command::Transpile(options) =
            parse(args(&["transpile", "-", "--loader=tsx", "--sourcemap"])).unwrap()
        else {
            panic!("expected transpile")
        };
        assert_eq!(options.loader.as_deref(), Some("tsx"));
        assert!(options.sourcemap);
        assert!(matches!(
            parse(args(&["help", "transpile"])).unwrap(),
            Command::Help(Some(Topic::Transpile))
        ));
        assert!(matches!(
            parse(args(&["--version"])).unwrap(),
            Command::Version
        ));
        assert!(parse(args(&["transpile", "file.ts", "--loader", "ts"])).is_err());
    }
}
