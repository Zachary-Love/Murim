//! Terminal colours from the user's default iTerm2 profile, so agents look the same as in their own terminal.
use serde::Serialize;
use serde_json::Value;
use std::process::Command;

#[derive(Serialize, Debug, PartialEq)]
pub struct Theme {
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    #[serde(rename = "selectionBackground")]
    pub selection: String,
    pub ansi: Vec<String>,
    /// iTerm "Normal Font", e.g. "Monaco 12" -> ("Monaco", 12); the name is a PostScript name WebKit resolves locally
    pub font: Option<String>,
    #[serde(rename = "fontSize")]
    pub font_size: Option<f64>,
}

fn hex(v: &Value) -> Option<String> {
    let c = |k: &str| v.get(format!("{k} Component")).and_then(Value::as_f64).map(|f| (f.clamp(0.0, 1.0) * 255.0).round() as u8);
    Some(format!("#{:02x}{:02x}{:02x}", c("Red")?, c("Green")?, c("Blue")?))
}

/// Pick the default profile (by guid, else the first) and its colours for the current appearance.
pub fn parse(bookmarks: &Value, default_guid: &str, dark: bool) -> Option<Theme> {
    let list = bookmarks.as_array()?;
    let b = list.iter().find(|b| b["Guid"] == default_guid).or(list.first())?;
    let suf = match (b["Use Separate Colors for Light and Dark Mode"].as_bool().unwrap_or(false), dark) {
        (false, _) => "",
        (true, true) => " (Dark)",
        (true, false) => " (Light)",
    };
    let col = |k: &str| b.get(format!("{k}{suf}")).or(b.get(k)).and_then(hex);
    Some(Theme {
        background: col("Background Color")?,
        foreground: col("Foreground Color")?,
        cursor: col("Cursor Color")?,
        selection: col("Selection Color")?,
        ansi: (0..16).map(|i| col(&format!("Ansi {i} Color"))).collect::<Option<_>>()?,
        font: b["Normal Font"].as_str().and_then(|f| f.rsplit_once(' ')).map(|(n, _)| n.to_string()),
        font_size: b["Normal Font"].as_str().and_then(|f| f.rsplit_once(' ')).and_then(|(_, s)| s.parse().ok()),
    })
}

/// None when iTerm2 isn't set up (the frontend keeps its built-in theme).
#[tauri::command]
pub fn terminal_theme() -> Option<Theme> {
    let home = std::env::var("HOME").ok()?;
    let plist = format!("{home}/Library/Preferences/com.googlecode.iterm2.plist");
    let out = Command::new("plutil").args(["-extract", "New Bookmarks", "json", "-o", "-", &plist]).output().ok()?;
    let bookmarks: Value = serde_json::from_slice(&out.stdout).ok()?;
    let guid = Command::new("defaults").args(["read", "com.googlecode.iterm2", "Default Bookmark Guid"]).output().ok()?;
    let dark = Command::new("defaults").args(["read", "-g", "AppleInterfaceStyle"]).output().map(|o| String::from_utf8_lossy(&o.stdout).trim() == "Dark").unwrap_or(false);
    parse(&bookmarks, String::from_utf8_lossy(&guid.stdout).trim(), dark)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn c(r: f64, g: f64, b: f64) -> Value {
        json!({"Red Component": r, "Green Component": g, "Blue Component": b})
    }

    #[test]
    fn picks_default_profile_and_appearance() {
        let mut p = json!({"Guid": "G2", "Use Separate Colors for Light and Dark Mode": true,
            "Background Color (Dark)": c(0.0, 0.0, 0.0), "Background Color (Light)": c(1.0, 1.0, 1.0),
            "Foreground Color": c(0.5, 0.5, 0.5), "Cursor Color": c(1.0, 0.0, 0.0), "Selection Color": c(0.0, 0.0, 1.0)});
        for i in 0..16 {
            p[format!("Ansi {i} Color")] = c(0.0, 1.0, 0.0);
        }
        p["Normal Font"] = json!("JetBrainsMono-Regular 13");
        let list = json!([{"Guid": "G1"}, p]);
        let d = parse(&list, "G2", true).unwrap();
        assert_eq!((d.background.as_str(), d.foreground.as_str(), d.ansi.len()), ("#000000", "#808080", 16));
        assert_eq!((d.font.as_deref(), d.font_size), (Some("JetBrainsMono-Regular"), Some(13.0)));
        assert_eq!(parse(&list, "G2", false).unwrap().background, "#ffffff");
        assert_eq!(parse(&list, "G1", true), None); // incomplete profile: keep the built-in theme
    }
}
