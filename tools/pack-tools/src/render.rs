//! `render-product template SRC DST` and `render-product plist DST VERSION REVISION`: product-facing
//! metadata from the identity (`PRODUCT_NAME`, `PRODUCT_SLUG`, `BUNDLE_ID`) in the environment.
use crate::{io, universal_newlines, Result};
use std::path::Path;

const KEYS: [&str; 3] = ["PRODUCT_NAME", "PRODUCT_SLUG", "BUNDLE_ID"];

fn env(get: &dyn Fn(&str) -> Option<String>, key: &str) -> Result<String> {
    get(key).ok_or_else(|| format!("environment variable {key} is not set"))
}

/// Replaces `@KEY@` for each identity key in turn (so a value that contains a later key is expanded too).
pub fn template_text(text: &str, get: &dyn Fn(&str) -> Option<String>) -> Result<String> {
    let mut text = universal_newlines(text);
    for key in KEYS {
        text = text.replace(&format!("@{key}@"), &env(get, key)?);
    }
    Ok(text)
}

pub fn template(
    source: &Path,
    destination: &Path,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<()> {
    let bytes = io(std::fs::read(source), source)?;
    let text = String::from_utf8(bytes).map_err(|e| format!("{}: {e}", source.display()))?;
    io(
        std::fs::write(destination, template_text(&text, get)?),
        destination,
    )
}

/// plistlib's text escaping.
fn escape(text: &str) -> String {
    universal_newlines(text)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The same XML property list `plistlib.dumps(values, sort_keys=False)` wrote.
pub fn plist_text(
    version: &str,
    revision: &str,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<String> {
    let name = env(get, "PRODUCT_NAME")?;
    let strings = [
        ("CFBundleName", name.clone()),
        ("CFBundleDisplayName", name),
        ("CFBundleIdentifier", env(get, "BUNDLE_ID")?),
        (
            "CFBundleExecutable",
            env(get, "PRODUCT_SLUG")? + "-launcher",
        ),
        ("CFBundlePackageType", "APPL".to_string()),
        ("CFBundleShortVersionString", version.to_string()),
        ("CFBundleVersion", revision.to_string()),
        ("LSMinimumSystemVersion", "12.0".to_string()),
    ];
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n",
    );
    for (key, value) in strings {
        out += &format!(
            "\t<key>{key}</key>\n\t<string>{}</string>\n",
            escape(&value)
        );
    }
    out += "\t<key>NSHighResolutionCapable</key>\n\t<true/>\n</dict>\n</plist>\n";
    Ok(out)
}

pub fn plist(
    destination: &Path,
    version: &str,
    revision: &str,
    get: &dyn Fn(&str) -> Option<String>,
) -> Result<()> {
    io(
        std::fs::write(destination, plist_text(version, revision, get)?),
        destination,
    )
}
