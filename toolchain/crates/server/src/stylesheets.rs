//! Project-installed Tailwind processes development CSS, including imports and URLs.
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

const SCRIPT: &str = r#"
const {createRequire} = require('node:module');
const path = require('node:path');
(async()=>{
  let input=''; for await(const part of process.stdin) input+=part;
  const {root,file}=JSON.parse(input);
  const req=createRequire(path.join(root,'package.json'));
  const tw=req('@tailwindcss/node');
  const {Scanner}=req('@tailwindcss/oxide');
  const dependencies=[];
  const compiler=await tw.compile(`@import ${JSON.stringify(file)};`,{base:root,shouldRewriteUrls:true,onDependency(file){dependencies.push(file)}});
  const sources=(compiler.root==='none'?[]:compiler.root===null?[{base:root,pattern:'**/*',negated:false}]:[{...compiler.root,negated:false}]).concat(compiler.sources);
  const scanner=new Scanner({sources});
  let css=compiler.build(scanner.scan());
  // Imported URLs are rebased by Tailwind relative to root. An injected style
  // must use root URLs rather than resolving them against the current SPA route.
  css=css.replace(/url\((['"]?)([^)'"\s]+)\1\)/g,(all,quote,url)=>{
    if(url.startsWith('/')||url.startsWith('#')||/^[a-z][a-z0-9+.-]*:/i.test(url))return all;
    return `url(${quote}/${url.replace(/^\.\//,'')}${quote})`;
  });
  process.stdout.write(JSON.stringify({css,dependencies}));
})().catch(e=>{console.error(e.stack||e);process.exit(1)});
"#;

/// Compiles a stylesheet with the application's installed Tailwind toolchain.
///
/// Runs a child process with JSON over stdin/stdout, collects imported dependencies
/// for graph invalidation, and rewrites relative asset URLs for injected styles.
/// Runtime discovery, process, compiler, and response errors propagate to the HTTP
/// pipeline instead of returning partially compiled CSS.
pub fn compile(root: &Path, file: &Path) -> Result<(String, Vec<String>), String> {
    let runtime = ["bun", "node"]
        .into_iter()
        .find(|bin| Command::new(bin).arg("--version").output().is_ok())
        .ok_or("CSS compilation needs Bun or Node on PATH")?;
    let mut child = Command::new(runtime)
        .args(["--eval", SCRIPT])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let request = serde_json::json!({"root":root,"file":file}).to_string();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(format!(
            "CSS compile {}: {}",
            file.display(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let result: serde_json::Value =
        serde_json::from_slice(&output.stdout).map_err(|e| e.to_string())?;
    let css = result["css"]
        .as_str()
        .ok_or("CSS compiler returned no CSS")?
        .to_string();
    let dependencies =
        serde_json::from_value(result["dependencies"].clone()).map_err(|e| e.to_string())?;
    Ok((css, dependencies))
}
