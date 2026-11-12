//! View > Ways to save space: what could be compressed (big, not changed
//! for a while) and other ways to get space back, all worked out from the
//! scan alone (no file is opened). Nothing here changes anything: each
//! suggestion leads to the usual dialog (Compress, delete review, a Windows
//! tool) or just explains what to do.
//!
//! "Not changed for a while" is the later of a file's last-modified date
//! and the day it arrived on this PC ([`Index::touched`]): downloads and
//! unpacked datasets keep their author's modified date, often years older
//! than the download. Windows keeps last-access dates only loosely (often
//! switched off, updated by virus scanners and backups), so Xylem doesn't
//! use them and never calls something "unused".
//!
//! [`Index::touched`]: xylem_core::Index::touched

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use xylem_core::classify::{Action, Safety};
use xylem_core::flags;

use crate::compress::{self, Level, Verdict};
use crate::{Scanned, safety, today_days};

/// The "Temp & caches" category (`CATEGORIES[6]`): those are cleared, not compressed.
const CAT_TEMP: u8 = 6;

/// Something that could be compressed: the highest folder (or a big file)
/// that hasn't changed for `min_days`, with what Xylem's engine would pack.
#[derive(Clone, Debug)]
pub struct Candidate {
    pub node: u32,
    pub path: String,
    pub dir: bool,
    /// Size on disk now.
    pub on_disk: u64,
    /// Size on disk of the files the engine would pack.
    pub packable: u64,
    /// Rough saving with Maximum (Fast: [`compress::fast_estimate`]).
    pub estimate_max: u64,
    /// Newest day a file inside was changed or arrived on this PC.
    /// Folders' own dates are left out (they change when a file is renamed
    /// or deleted inside).
    pub mtime: u16,
}

impl Candidate {
    pub fn estimate(&self, level: Level) -> u64 {
        match level {
            Level::Max => self.estimate_max,
            Level::Fast => compress::fast_estimate(self.estimate_max),
        }
    }

    /// Maximum for things nobody has touched for a year, otherwise Fast.
    pub fn suggested(&self, today: u16) -> Level {
        if today.saturating_sub(self.mtime) >= 365 { Level::Max } else { Level::Fast }
    }
}

#[derive(Clone, Debug)]
pub struct Suggestions {
    pub items: Vec<Candidate>,
    pub min_days: u16,
    pub min_saving: u64,
    /// Why nothing on this drive can be compressed (not NTFS), if so.
    pub not_possible: Option<String>,
    pub secs: f64,
}

impl Suggestions {
    pub fn total(&self, level: Level) -> u64 {
        self.items.iter().map(|c| c.estimate(level)).sum()
    }
}

/// What a tip's button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TipAction {
    Tool(Action),
    Similar,
    Leftovers,
    Dupes,
}

/// One place a tip is about.
#[derive(Clone, Debug)]
pub struct Place {
    pub node: u32,
    pub path: String,
    pub bytes: u64,
    pub mtime: u16,
    /// The rule's note for the place (e.g. its cleanup command).
    pub note: Option<String>,
    /// Safe to clear (a built-in green rule matched it, nothing protected):
    /// can be marked for the usual delete review.
    pub markable: bool,
    /// A WSL/Docker virtual disk: offer the Compact dialog.
    pub vhdx: bool,
}

/// One other way to save space.
#[derive(Clone, Debug)]
pub struct Tip {
    pub title: String,
    /// What it could give back; None when the scan can't tell.
    pub bytes: Option<u64>,
    pub how: String,
    pub places: Vec<Place>,
    pub button: Option<(&'static str, TipAction)>,
}

impl Scanned {
    /// View > Ways to save space > Compress: folders and big files whose
    /// newest file is at least `min_days` old and where Xylem's engine would
    /// save at least `min_saving` (rough estimate, Maximum). The highest
    /// such folder is listed, not everything inside it. Same rules as the
    /// Compress dialog: protected places, cloud folders, links and caches
    /// are left out. Biggest saving first.
    pub fn compress_suggestions(&self, min_days: u16, min_saving: u64, limit: usize) -> Suggestions {
        let t = Instant::now();
        let idx = &self.index;
        let n = idx.len();
        let mut out = Suggestions { items: Vec::new(), min_days, min_saving, not_possible: None, secs: 0.0 };
        if let Err(e) = compress::check_volume(Path::new(&idx.path(0))) {
            out.not_possible = Some(e);
            return out;
        }
        // What the engine would pack and save under each node. Children have
        // higher ids than their parent, so one reverse pass rolls it up.
        // Folders the Compress dialog wouldn't go into (links, protected
        // places) add nothing to their parent, as in `compress_plan`.
        let mut est = vec![0u64; n];
        let mut pack = vec![0u64; n];
        let mut newest = vec![0u16; n];
        for i in (1..n).rev() {
            let id = i as u32;
            let fl = idx.flags(id);
            if fl & flags::DELETED != 0 {
                continue;
            }
            if idx.is_dir(id) {
                if fl & flags::LINK != 0 || self.cls.protected_here(&self.rules, id).is_some() {
                    continue;
                }
            } else {
                // Every file counts for the date (a folder whose small files
                // change isn't left alone), only packable ones for the saving.
                newest[i] = idx.touched(id);
                if let Verdict::Pack(e) = compress::verdict(idx.name(id), fl, idx.size(id), idx.alloc(id)) {
                    est[i] = e as u64;
                    pack[i] = idx.alloc(id);
                }
            }
            let p = idx.parent(id) as usize;
            est[p] = est[p].saturating_add(est[i]);
            pack[p] = pack[p].saturating_add(pack[i]);
            newest[p] = newest[p].max(newest[i]);
        }
        let today = today_days();
        let cold = |m: u16| m != 0 && today.saturating_sub(m) >= min_days;
        let mut stack: Vec<u32> = self.children.live(idx, 0).collect();
        while let Some(x) = stack.pop() {
            let i = x as usize;
            if est[i] < min_saving || self.cls.in_protected_tree(x) || idx.flags(x) & (flags::LINK | flags::CLOUD) != 0 {
                continue;
            }
            let path = idx.path(x);
            if safety::in_cloud_folder(Path::new(&path)) {
                continue;
            }
            let dir = idx.is_dir(x);
            let ok = cold(newest[i]) && self.cls.cat[i] != CAT_TEMP && self.cls.protection(idx, &self.rules, x).is_none() && path.trim_end_matches('\\').len() > 2;
            if ok {
                out.items.push(Candidate { node: x, path, dir, on_disk: idx.alloc(x), packable: pack[i], estimate_max: est[i], mtime: newest[i] });
            } else if dir {
                stack.extend(self.children.live(idx, x));
            }
        }
        out.items.sort_unstable_by_key(|c| std::cmp::Reverse(c.estimate_max));
        out.items.truncate(limit);
        out.secs = t.elapsed().as_secs_f64();
        out
    }

    /// View > Ways to save space > Other ways: caches, old build folders,
    /// models, Windows' own cleanups, old files that won't compress, local
    /// OneDrive copies, and pointers to the finders. Biggest first; tips the
    /// scan can't size come last.
    pub fn space_tips(&self) -> Vec<Tip> {
        let idx = &self.index;
        let today = today_days();
        let mut tips = Vec::new();

        let mut caches = self.places_for(
            &["Package caches", "Browser cache", "GPU shader cache", "Thumbnail cache", "User temp", "Windows temp", "App caches", "Media caches", "Crash dumps", "Error reports", "Windows Update cache", "PyTorch cache", "Keras cache", "Gradle"],
            |_| true,
        );
        // A folder named "cache" inside the user's own files (a project's
        // cache, not an app's) is probably rebuildable, but maybe only by
        // hours of work: never offered as "safe".
        for p in &mut caches {
            if xylem_core::classify::in_own_files(&p.path) {
                p.markable = false;
                p.note = Some("A cache folder inside your own files, not an app's. Check what made it before clearing it: rebuilding a project's cache can take long.".into());
            }
        }
        tips.push(tip(
            "Caches and temporary files",
            caches,
            "Apps rebuild these when they need them, so clearing them is safe; the first use afterwards is a little slower. Hover a place for its own cleanup command. Settings > Storage > Temporary files clears the Windows ones.",
            Some(("Open Storage settings", TipAction::Tool(Action::StorageSettings))),
        ));

        let dev = self.places_for(&["node_modules", "Build output", "Python environments"], |p| p.mtime != 0 && today.saturating_sub(p.mtime) >= 180);

        tips.push(tip(
            "Build folders in projects not changed for 6 months",
            dev,
            "Folders a build or package manager makes again when you need them (npm install, cargo build, pip install -r requirements.txt). The project's own files are not included.",
            None,
        ));

        // Only the model stores: "Model files" (found by extension) would
        // list every folder holding a .bin or .pt, even the Recycle Bin.
        let models = self.places_for(&["Ollama models", "LM Studio models", "Hugging Face cache"], |_| true);
        tips.push(tip(
            "AI models and model caches",
            models,
            "Remove models you no longer use with the tool that downloaded them (ollama rm <model>, huggingface-cli delete-cache, LM Studio's My Models), so it doesn't think they're still there. The date shows when each was last changed.",
            None,
        ));

        let bin = self.places_for(&["Recycle Bin"], |_| true);
        tips.push(tip("Recycle Bin", bin, "Files you deleted, still taking space. Empty it: right-click the Recycle Bin on the desktop > Empty Recycle Bin.", None));

        let hib = self.places_for(&["Hibernation file"], |_| true);
        tips.push(tip(
            "Hibernation file",
            hib,
            "In a terminal opened as administrator: 'powercfg /h /type reduced' makes it about half as big and keeps Fast startup; 'powercfg /h off' removes it, but turns off hibernate and Fast startup.",
            None,
        ));

        let old_win = self.places_for(&["Previous Windows", "Windows upgrade files"], |_| true);
        tips.push(tip(
            "Previous Windows and upgrade files",
            old_win,
            "Windows removes these by itself after about 10 days, or now from Settings > Storage > Temporary files > Previous Windows installation(s). You can't go back to the older Windows afterwards.",
            Some(("Open Storage settings", TipAction::Tool(Action::StorageSettings))),
        ));

        let restore = self.places_for(&["Restore points & shadow copies"], |_| true);
        tips.push(tip(
            "Restore points",
            restore,
            "System Protection > Configure lets you lower how much space restore points may use, or delete them all. Keep at least one if you rely on them.",
            Some(("Open System Protection", TipAction::Tool(Action::SystemProtection))),
        ));

        // One pass over the files: old ones that won't compress, and virtual disks.
        let (mut big_cold, mut big_cold_bytes, mut vhdx) = (Vec::new(), 0u64, Vec::new());
        for i in 1..idx.len() {
            let id = i as u32;
            let fl = idx.flags(id);
            if idx.is_dir(id) || fl & (flags::DELETED | flags::CLOUD | flags::LINK) != 0 || self.cls.in_protected_tree(id) {
                continue;
            }
            let a = idx.alloc(id);
            let name = idx.name(id);
            let ext = name.rsplit_once('.').map_or("", |e| e.1).to_ascii_lowercase();
            if (ext == "vhdx" || ext == "vhd") && a >= 1 << 30 {
                vhdx.push(self.place(id, false, true));
                continue;
            }
            let m = idx.touched(id);
            if a >= 512 << 20 && m != 0 && today.saturating_sub(m) >= 365 && compress::verdict(name, fl, idx.size(id), a) == Verdict::Format {
                big_cold_bytes += a;
                big_cold.push(self.place(id, false, false));
            }
        }
        big_cold.sort_unstable_by_key(|p| std::cmp::Reverse(p.bytes));
        big_cold.truncate(50);
        if !big_cold.is_empty() {
            tips.push(Tip {
                title: format!("Big files not changed for a year that won't compress ({})", big_cold.len()),
                bytes: Some(big_cold_bytes),
                how: "Videos, archives, disk images and other packed files of 512 MB or more. Compressing them saves nothing; moving them to an external drive or cloud storage would.".into(),
                places: big_cold,
                button: None,
            });
        }
        if !vhdx.is_empty() {
            vhdx.sort_unstable_by_key(|p| std::cmp::Reverse(p.bytes));
            tips.push(Tip {
                title: "WSL and Docker virtual disks".into(),
                bytes: None,
                how: "These disk files grow but never shrink by themselves. Free space inside first (delete files, 'docker system prune'), then Compact gives it back to Windows. How much depends on what's free inside, which the scan can't see.".into(),
                places: vhdx,
                button: None,
            });
        }

        if let Some(od) = self.onedrive_local(today, 180) {
            tips.push(od);
        }

        let similar = self.similar_folders();
        let copies: u64 = similar.copies.iter().map(|g| g.reclaim).sum();
        if copies > 0 {
            tips.push(Tip {
                title: format!("Folders that look like copies ({})", similar.copies.len()),
                bytes: Some(copies),
                how: "Same size, file count and names inside (contents aren't compared). Check what a copy is before removing it.".into(),
                places: Vec::new(),
                button: Some(("Open Similar folders", TipAction::Similar)),
            });
        }
        let downloads: Vec<_> = self.leftovers(300).into_iter().filter(|l| l.action.is_none()).collect();
        if !downloads.is_empty() {
            tips.push(Tip {
                title: format!("Installers, archives and disk images in Downloads ({})", downloads.len()),
                bytes: Some(downloads.iter().map(|l| l.on_disk).sum()),
                how: "Often kept after installing. Only you can tell whether you still need one.".into(),
                places: Vec::new(),
                button: Some(("Open Leftovers", TipAction::Leftovers)),
            });
        }
        tips.push(Tip {
            title: "Duplicate files".into(),
            bytes: None,
            how: "Files with exactly the same content in two or more places. The finder reads them to be sure, so it isn't run until you ask.".into(),
            places: Vec::new(),
            button: Some(("Find duplicates", TipAction::Dupes)),
        });

        if let Some(win) = self.places_for(&["Component store (WinSxS)"], |_| true).first() {
            tips.push(Tip {
                title: "Windows component store (WinSxS)".into(),
                bytes: None,
                how: format!(
                    "It looks like {}, but most of it is shared with C:\\Windows, so the real saving is much smaller. Windows' DISM cleanup removes old component versions.",
                    xylem_core::format::bytes(win.bytes)
                ),
                places: Vec::new(),
                button: Some(("Clean up with DISM", TipAction::Tool(Action::DismCleanup))),
            });
            tips.push(Tip {
                title: "Let Windows compress itself (Compact OS)".into(),
                bytes: None,
                how: "Windows can keep its own files compressed; Microsoft quotes about 2.6 GB saved on 64-bit Windows. In a terminal opened as administrator: 'compact /compactos:query' shows whether it's on, 'compact /compactos:always' turns it on. Xylem doesn't compress C:\\Windows itself.".into(),
                places: Vec::new(),
                button: None,
            });
        }

        tips.retain(|t| t.bytes != Some(0));
        // Sized tips biggest first, then the ones the scan can't size, in the order above.
        tips.sort_by_key(|t| std::cmp::Reverse(t.bytes.map_or(0, |b| b.saturating_add(1))));
        tips
    }

    /// The places of these sub-types that pass `keep`, biggest first, with
    /// places inside another listed place left out (so nothing is counted
    /// or marked twice).
    fn places_for(&self, subs: &[&str], keep: impl Fn(&Place) -> bool) -> Vec<Place> {
        let idx = &self.index;
        let mut nodes = Vec::new();
        for name in subs {
            let Some(s) = self.rules.subs.iter().position(|x| x.name == *name) else { continue };
            for p in &self.cats.places_by_sub[s] {
                if idx.flags(p.node) & flags::DELETED == 0 && !nodes.contains(&p.node) {
                    nodes.push(p.node);
                }
            }
        }
        let set: HashSet<u32> = nodes.iter().copied().collect();
        let mut out: Vec<Place> = nodes
            .into_iter()
            .filter(|&n| {
                let mut cur = idx.parent(n);
                while cur != xylem_core::NO_PARENT {
                    if set.contains(&cur) {
                        return false;
                    }
                    cur = idx.parent(cur);
                }
                true
            })
            .map(|n| self.place(n, true, false))
            .filter(|p| p.bytes > 0 && keep(p))
            .collect();
        out.sort_unstable_by_key(|p| std::cmp::Reverse(p.bytes));
        out
    }

    fn place(&self, n: u32, with_note: bool, vhdx: bool) -> Place {
        let r = self.cls.rule[n as usize];
        let info = self.rules.info.get(r as usize);
        Place {
            node: n,
            path: self.index.path(n),
            bytes: self.index.alloc(n),
            mtime: self.index.touched(n),
            note: if with_note { info.and_then(|i| i.note.clone()) } else { None },
            markable: n != 0
                && self.cls.matched_here(n)
                && info.is_some_and(|i| xylem_core::classify::safety_at(i, &self.index.path(n)) == Some(Safety::Green) && !i.user)
                && self.cls.protection(&self.index, &self.rules, n).is_none(),
            vhdx,
        }
    }

    /// Files in OneDrive folders that are kept on this PC (not online-only)
    /// and haven't changed for `min_days`.
    fn onedrive_local(&self, today: u16, min_days: u16) -> Option<Tip> {
        let idx = &self.index;
        let roots = self.places_for(&["OneDrive"], |_| true);
        let mut places = Vec::new();
        let mut total = 0u64;
        for mut p in roots {
            let mut bytes = 0u64;
            let mut stack = vec![p.node];
            while let Some(x) = stack.pop() {
                let fl = idx.flags(x);
                if fl & (flags::DELETED | flags::LINK) != 0 {
                    continue;
                }
                if idx.is_dir(x) {
                    stack.extend(self.children.live(idx, x));
                } else if fl & flags::CLOUD == 0 && idx.touched(x) != 0 && today.saturating_sub(idx.touched(x)) >= min_days {
                    bytes += idx.alloc(x);
                }
            }
            if bytes > 0 {
                total += bytes;
                p.bytes = bytes;
                p.note = None;
                p.markable = false;
                places.push(p);
            }
        }
        (total > 0).then(|| Tip {
            title: "OneDrive files kept on this PC, not changed for 6 months".into(),
            bytes: Some(total),
            how: "OneDrive keeps a copy here as well as online. In Explorer, right-click a folder > Free up space: the files stay in OneDrive and download again when you open them. Deleting them would delete them from OneDrive too.".into(),
            places,
            button: None,
        })
    }
}

fn tip(title: &str, places: Vec<Place>, how: &str, button: Option<(&'static str, TipAction)>) -> Tip {
    let bytes = places.iter().map(|p| p.bytes).sum();
    Tip { title: title.into(), bytes: Some(bytes), how: how.into(), places, button }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xylem_core::{Index, ScanStats};

    const MB: u64 = 1 << 20;

    fn file(idx: &mut Index, parent: u32, name: &str, bytes: u64, days: u16) -> u32 {
        let f = idx.push(parent, name, bytes, bytes, 0);
        idx.set_mtime(f, days);
        f
    }

    #[test]
    fn suggests_the_highest_unchanged_folder_and_big_files() {
        let today = today_days();
        let (old, new) = (today - 400, today - 2);
        let mut idx = Index::new("C:\\");
        let data = idx.push(0, "data", 0, 0, flags::DIR);
        // Cold project: listed once, as a whole (not its subfolders).
        let cold = idx.push(data, "old-project", 0, 0, flags::DIR);
        // Copied here recently: the folder's own date is new, its files are old.
        idx.set_mtime(cold, new);
        let sub = idx.push(cold, "logs", 0, 0, flags::DIR);
        file(&mut idx, sub, "run.log", 600 * MB, old);
        file(&mut idx, cold, "notes.txt", 100 * MB, old);
        file(&mut idx, cold, "clip.mp4", 900 * MB, old);
        // Downloaded last week, but the files keep their authors' old dates: not listed.
        let fresh = idx.push(data, "new-download", 0, 0, flags::DIR);
        let dl = file(&mut idx, fresh, "eeg.edf", 900 * MB, old);
        idx.set_added(dl, today - 7);
        // Hot folder: not listed, but its big unchanged file is.
        let hot = idx.push(data, "work", 0, 0, flags::DIR);
        file(&mut idx, hot, "today.txt", 4096 * 2, new);
        let big = file(&mut idx, hot, "dump.sql", 1024 * MB, old);
        // Too small, already compressed, a cache, a protected place: never.
        let small = idx.push(data, "small", 0, 0, flags::DIR);
        file(&mut idx, small, "a.txt", 10 * MB, old);
        let packed = idx.push(data, "packed", 0, 0, flags::DIR);
        let p = idx.push(packed, "a.txt", 800 * MB, 100 * MB, 0);
        idx.set_mtime(p, old);
        let users = idx.push(0, "Users", 0, 0, flags::DIR);
        let me = idx.push(users, "me", 0, 0, flags::DIR);
        let ad = idx.push(me, "AppData", 0, 0, flags::DIR);
        let local = idx.push(ad, "Local", 0, 0, flags::DIR);
        let tmp = idx.push(local, "Temp", 0, 0, flags::DIR);
        file(&mut idx, tmp, "big.tmp", 800 * MB, old);
        let win = idx.push(0, "Windows", 0, 0, flags::DIR);
        file(&mut idx, win, "x.dll", 800 * MB, old);
        idx.compute_totals();
        let s = Scanned::new(idx, ScanStats::default(), "test".into());

        let got = s.compress_suggestions(180, 64 * MB, 100);
        assert!(got.not_possible.is_none());
        let ids: Vec<u32> = got.items.iter().map(|c| c.node).collect();
        assert_eq!(ids, vec![big, cold], "{:?}", got.items.iter().map(|c| &c.path).collect::<Vec<_>>());
        let c = &got.items[1];
        assert_eq!(c.packable, 700 * MB, "the video is left out");
        assert_eq!(c.estimate_max, 350 * MB);
        assert_eq!(c.estimate(Level::Fast), compress::fast_estimate(350 * MB));
        assert_eq!(c.suggested(today), Level::Max);
        // Once the download is old enough, it's listed with its arrival date.
        let later = s.compress_suggestions(5, 64 * MB, 100);
        let dl_row = later.items.iter().find(|c| c.node == fresh).expect("listed at 5 days");
        assert_eq!(dl_row.mtime, today - 7);
        assert_eq!(c.on_disk, 1600 * MB);
        // A shorter "unchanged for" (but longer than a week) doesn't change it; a longer one finds nothing.
        assert_eq!(s.compress_suggestions(30, 64 * MB, 100).items.len(), 2);
        assert!(s.compress_suggestions(500, 64 * MB, 100).items.is_empty());
        // The Compress dialog agrees on what it would pack.
        let plan = s.compress_plan(cold, false).unwrap();
        assert_eq!((plan.bytes, plan.estimate_max), (c.packable, c.estimate_max));
    }

    #[test]
    fn measured_ratios_for_data_files() {
        assert_eq!(compress::ratio("npy"), 0.13);
        assert_eq!(compress::ratio("PT"), 0.07);
        assert_eq!(compress::ratio("gguf"), 0.0);
        assert_eq!(compress::ratio("log"), 0.5);
        assert_eq!(compress::verdict("a.npz", 0, 100 * MB, 100 * MB), Verdict::Pack(0.13 * (100 * MB) as f64));
    }

    #[test]
    fn tips_count_old_build_folders_once_and_skip_fresh_ones() {
        let today = today_days();
        let (old, new) = (today - 400, today - 2);
        let mut idx = Index::new("C:\\");
        let users = idx.push(0, "Users", 0, 0, flags::DIR);
        let me = idx.push(users, "me", 0, 0, flags::DIR);
        let a = idx.push(me, "old-app", 0, 0, flags::DIR);
        let nm = idx.push(a, "node_modules", 0, 0, flags::DIR);
        file(&mut idx, nm, "x.js", 30 * MB, old);
        let inner = idx.push(nm, "pkg", 0, 0, flags::DIR);
        let inner_nm = idx.push(inner, "node_modules", 0, 0, flags::DIR);
        file(&mut idx, inner_nm, "y.js", 20 * MB, old);
        let b = idx.push(me, "new-app", 0, 0, flags::DIR);
        let nm2 = idx.push(b, "node_modules", 0, 0, flags::DIR);
        file(&mut idx, nm2, "z.js", 40 * MB, new);
        let proj_cache = idx.push(a, "cache", 0, 0, flags::DIR);
        file(&mut idx, proj_cache, "features.bin", 70 * MB, old);
        let ad = idx.push(me, "AppData", 0, 0, flags::DIR);
        let local = idx.push(ad, "Local", 0, 0, flags::DIR);
        let app = idx.push(local, "SomeApp", 0, 0, flags::DIR);
        let app_cache = idx.push(app, "Cache", 0, 0, flags::DIR);
        file(&mut idx, app_cache, "c.bin", 60 * MB, new);
        let vids = idx.push(me, "Videos", 0, 0, flags::DIR);
        let movie = file(&mut idx, vids, "trip.mp4", 2048 * MB, old);
        file(&mut idx, vids, "recent.mp4", 2048 * MB, new);
        idx.compute_totals();
        let s = Scanned::new(idx, ScanStats::default(), "test".into());
        let tips = s.space_tips();
        let find = |t: &str| tips.iter().find(|x| x.title.starts_with(t)).unwrap_or_else(|| panic!("no tip {t}"));
        let dev = find("Build folders");
        assert_eq!(dev.places.iter().map(|p| p.node).collect::<Vec<_>>(), vec![nm], "outer one only, fresh one left out");
        assert_eq!(dev.bytes, Some(50 * MB));
        assert!(dev.places[0].markable);
        let caches = find("Caches");
        let safe: Vec<(u32, bool)> = caches.places.iter().map(|p| (p.node, p.markable)).collect();
        assert_eq!(safe, vec![(proj_cache, false), (app_cache, true)], "a project's own cache is never offered as safe");
        assert!(caches.places[0].note.as_deref().unwrap().contains("your own files"));
        // Same answer everywhere (details pane, Mark all): "check first" in a project, "safe" in AppData.
        assert_eq!((s.safety(proj_cache), s.safety(app_cache)), (Some(Safety::Yellow), Some(Safety::Green)));
        let all_caches = s.rules().subs.iter().position(|x| x.name == "App caches").unwrap();
        assert_eq!(s.markable(&format!("sub:{all_caches}")), vec![app_cache], "Mark all leaves the project's cache alone");
        let big = find("Big files not changed");
        assert_eq!(big.places.iter().map(|p| p.node).collect::<Vec<_>>(), vec![movie]);
        assert!(tips.iter().all(|t| t.bytes != Some(0)), "empty tips are dropped");
        // Sized tips first, biggest first.
        let sized: Vec<u64> = tips.iter().map_while(|t| t.bytes).collect();
        assert!(sized.windows(2).all(|w| w[0] >= w[1]));
        assert!(tips[sized.len()..].iter().all(|t| t.bytes.is_none()));
    }

    /// Real-data check (prints, changes nothing). Set XYLEM_SAVINGS_PATH
    /// (default C:) and run with --release --ignored --nocapture.
    #[test]
    #[ignore = "scans a real drive"]
    fn real_drive() {
        let path = std::env::var("XYLEM_SAVINGS_PATH").unwrap_or_else(|_| "C:\\".into());
        let s = crate::scan(&path).unwrap();
        let today = today_days();
        let b = xylem_core::format::bytes;
        for days in [30, 90, 180, 365] {
            let t = Instant::now();
            let got = s.compress_suggestions(days, 256 * MB, 100);
            println!(
                "\nCompress, unchanged {days}+ days: {} items, Fast ~{} / Max ~{}, {:.3} s ({:?})",
                got.items.len(),
                b(got.total(Level::Fast)),
                b(got.total(Level::Max)),
                t.elapsed().as_secs_f64(),
                got.not_possible
            );
            for c in got.items.iter().take(15) {
                println!(
                    "  {:>9} on disk  Fast ~{:>9}  Max ~{:>9}  {:<14} {:?}  {}",
                    b(c.on_disk),
                    b(c.estimate(Level::Fast)),
                    b(c.estimate(Level::Max)),
                    crate::age_text(today, c.mtime),
                    c.suggested(today),
                    c.path
                );
            }
        }
        if let Ok(probe) = std::env::var("XYLEM_SAVINGS_PROBE") {
            let idx = &s.index;
            let mut n = s.find_rel(&probe).expect("probe path in scan");
            loop {
                println!("  probe {:<60} mtime {:<14} dir {}", idx.path(n), crate::age_text(today, idx.mtime(n)), idx.is_dir(n));
                if n == 0 {
                    break;
                }
                n = idx.parent(n);
            }
            let start = s.find_rel(&probe).unwrap();
            let mut newest = (0u16, String::new());
            let mut stack = vec![start];
            while let Some(x) = stack.pop() {
                if idx.is_dir(x) {
                    stack.extend(s.children.live(idx, x));
                } else if idx.mtime(x) > newest.0 {
                    newest = (idx.mtime(x), idx.path(x));
                }
            }
            println!("  newest file inside: {} {}", crate::age_text(today, newest.0), newest.1);
        }
        let t = Instant::now();
        let tips = s.space_tips();
        println!("\nOther ways ({:.3} s):", t.elapsed().as_secs_f64());
        for tip in &tips {
            println!("  {:>9}  {}  ({} places)", tip.bytes.map_or("?".into(), b), tip.title, tip.places.len());
            for p in tip.places.iter().take(3) {
                println!("             {:>9}  {}", b(p.bytes), p.path);
            }
        }
    }
}

