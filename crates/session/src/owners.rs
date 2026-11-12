//! Which installed program or game a folder belongs to ("Belongs to" in
//! the details pane). Sources, all read-only and local:
//! - Windows' list of installed programs (Uninstall keys: InstallLocation),
//! - Steam's library list and game manifests (`libraryfolders.vdf`,
//!   `appmanifest_*.acf`),
//! - Epic Games Launcher manifests (`ProgramData\Epic\...\Manifests\*.item`).

use std::path::Path;

/// (lowercase path without a trailing backslash, label), longest first.
#[derive(Debug, Default)]
pub struct Owners {
    entries: Vec<(String, String)>,
}

fn norm(p: &str) -> String {
    p.trim().trim_matches('"').replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Too broad to name one owner ("C:\", "C:\Program Files", "D:\Games").
fn too_broad(p: &str) -> bool {
    let comps = p.split('\\').filter(|c| !c.is_empty()).count();
    comps <= 1
        || (comps == 2
            && ["program files", "program files (x86)", "programdata", "windows", "users", "games", "apps", "tools"]
                .iter()
                .any(|b| p.ends_with(&format!("\\{b}"))))
}

/// Values of `"key"  "value"` lines in a Valve KeyValues file.
fn vdf_values<'a>(text: &'a str, key: &str) -> Vec<&'a str> {
    let quoted = format!("\"{key}\"");
    text.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix(&quoted)?.trim();
            rest.strip_prefix('"')?.strip_suffix('"')
        })
        .collect()
}

impl Owners {
    pub fn from_entries(mut entries: Vec<(String, String)>) -> Self {
        entries.retain(|(p, l)| !p.is_empty() && !l.trim().is_empty() && !too_broad(p));
        entries.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
        entries.dedup_by(|a, b| a.0 == b.0);
        Self { entries }
    }

    /// Reads the sources. A few milliseconds to a few hundred (big Steam libraries).
    pub fn load() -> Self {
        let mut e: Vec<(String, String)> =
            crate::orphans::install_locations().into_iter().map(|(p, n)| (norm(&p), format!("Installed program: {}", n.trim()))).collect();
        // Steam: libraries, then each game's folder under steamapps\common.
        if let Some(steam) = crate::orphans::user_string(r"Software\Valve\Steam", "SteamPath") {
            let steam = norm(&steam);
            let mut libs = vec![steam.clone()];
            if let Ok(t) = std::fs::read_to_string(Path::new(&steam).join("steamapps").join("libraryfolders.vdf")) {
                libs.extend(vdf_values(&t, "path").into_iter().map(|p| norm(&p.replace("\\\\", "\\"))));
            }
            libs.sort();
            libs.dedup();
            for lib in libs {
                let Ok(dir) = std::fs::read_dir(Path::new(&lib).join("steamapps")) else { continue };
                for f in dir.flatten() {
                    let name = f.file_name().to_string_lossy().to_lowercase();
                    if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                        continue;
                    }
                    let Ok(t) = std::fs::read_to_string(f.path()) else { continue };
                    if let (Some(game), Some(dir)) = (vdf_values(&t, "name").first(), vdf_values(&t, "installdir").first()) {
                        e.push((format!("{lib}\\steamapps\\common\\{}", dir.to_lowercase()), format!("Steam game: {game}")));
                    }
                }
            }
        }
        // Epic Games Launcher.
        let pd = std::env::var("ProgramData").unwrap_or_else(|_| r"C:\ProgramData".into());
        if let Ok(dir) = std::fs::read_dir(Path::new(&pd).join(r"Epic\EpicGamesLauncher\Data\Manifests")) {
            for f in dir.flatten() {
                if !f.file_name().to_string_lossy().to_lowercase().ends_with(".item") {
                    continue;
                }
                let Some(v) = std::fs::read_to_string(f.path()).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { continue };
                if let (Some(loc), Some(name)) = (v["InstallLocation"].as_str(), v["DisplayName"].as_str()) {
                    e.push((norm(loc), format!("Epic Games: {name}")));
                }
            }
        }
        Self::from_entries(e)
    }

    /// The owner of `path` (the most specific one), if any.
    pub fn of(&self, path: &str) -> Option<&str> {
        let p = norm(path);
        self.entries.iter().find(|(e, _)| p == *e || (p.starts_with(e.as_str()) && p.as_bytes().get(e.len()) == Some(&b'\\'))).map(|(_, l)| l.as_str())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn most_specific_owner_wins_and_broad_folders_are_ignored() {
        let o = Owners::from_entries(vec![
            (norm(r"C:\Program Files"), "Installed program: Too broad".into()),
            (norm(r"D:\"), "Installed program: Whole drive".into()),
            (norm(r"C:\Program Files\Microsoft Visual Studio\2022\Community\"), "Installed program: Visual Studio".into()),
            (norm(r"D:\SteamLibrary\steamapps\common\RE4"), "Steam game: Resident Evil 4".into()),
            (norm(r"C:\Program Files\Microsoft Visual Studio"), "Installed program: VS installer".into()),
        ]);
        assert_eq!(o.len(), 3);
        assert_eq!(o.of(r"c:\program files\microsoft visual studio\2022\community\common7\ide"), Some("Installed program: Visual Studio"));
        assert_eq!(o.of(r"C:\Program Files\Microsoft Visual Studio\Shared"), Some("Installed program: VS installer"));
        assert_eq!(o.of(r"D:\SteamLibrary\steamapps\common\RE4"), Some("Steam game: Resident Evil 4"));
        assert_eq!(o.of(r"D:\SteamLibrary\steamapps\common\RE4 Mods"), None, "a prefix of the name isn't inside");
        assert_eq!(o.of(r"C:\Program Files\Other"), None);
    }

    #[test]
    fn reads_valve_keyvalues() {
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"2050650\"\n\t\"name\"\t\t\"Resident Evil 4\"\n\t\"installdir\"\t\t\"RE4\"\n}\n";
        assert_eq!(vdf_values(acf, "name"), ["Resident Evil 4"]);
        assert_eq!(vdf_values(acf, "installdir"), ["RE4"]);
        let lib = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(vdf_values(lib, "path"), ["C:\\\\Program Files (x86)\\\\Steam", "D:\\\\SteamLibrary"]);
    }

    #[test]
    #[ignore = "reads this PC's installed programs"]
    fn real_owners() {
        let o = Owners::load();
        eprintln!("{} owners", o.len());
        assert!(!o.is_empty());
    }
}
