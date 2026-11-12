//! Known offenders: programs and drive firmware known to wear SSDs out
//! fast. Ships inside the app (no lookups online). Every entry was checked
//! against public reports, listed in `sources`, with the date checked.
//! Xylem only explains: it never stops a service or changes firmware.

use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct Offender {
    /// Process file name (lowercase), or empty for a firmware entry.
    pub process: &'static str,
    /// Drive model must contain this, and firmware must equal `firmware`.
    pub model: &'static str,
    pub firmware: &'static str,
    pub title: &'static str,
    pub what: &'static str,
    pub todo: &'static str,
    pub sources: &'static [&'static str],
    pub checked: &'static str,
}

pub const LIST: &[Offender] = &[
    Offender {
        process: "atieclxx.exe",
        model: "",
        firmware: "",
        title: "AMD External Events Utility (Radeon driver)",
        what: "A bug AMD confirmed in Nov 2026: on some Radeon PCs it keeps writing to the system drive even when the PC is idle (reports of about 0.3-5 MB/s), which wears the SSD.",
        todo: "Update the AMD Radeon driver (AMD said the fix comes with Adrenalin 26.11.1). Stopping the \"AMD External Events Utility\" service also stops it, but can break Radeon hotkeys and some display features. Xylem doesn't change anything here.",
        sources: &[
            "https://www.techspot.com/news/114496-amd-confirms-gpu-driver-bug-writing-data-continuously.html",
            "https://hwbusters.com/news/amd-confirms-radeon-driver-bug-that-keeps-ssds-writing-at-idle-fix-due-in-adrenalin-26-11-1/",
            "https://www.guru3d.com/story/amd-radeon-driver-bug-linked-to-continuous-ssd-writes-fix-is-planned/",
        ],
        checked: "2026-11-12",
    },
    Offender {
        process: "",
        model: "Samsung SSD 990 PRO",
        firmware: "0B2QJXD7",
        title: "Samsung 990 PRO, first firmware",
        what: "With firmware 0B2QJXD7, 990 PRO drives reported their health dropping unusually fast (early 2023).",
        todo: "Update the firmware to 1B2QJXD7 or newer with Samsung Magician. The update stops the fast decline but doesn't restore health already lost.",
        sources: &[
            "https://www.tomshardware.com/news/samsung-990-pro-firmware-update-released-ssd-health",
            "https://www.pugetsystems.com/blog/2023/03/14/samsung-990-pro-critical-firmware-update/",
        ],
        checked: "2026-11-12",
    },
];

/// The entry for a running program, by file name.
pub fn for_process(name: &str) -> Option<&'static Offender> {
    LIST.iter().find(|o| !o.process.is_empty() && o.process.eq_ignore_ascii_case(name.trim()))
}

/// The entry for a drive model + firmware.
pub fn for_drive(model: &str, firmware: &str) -> Option<&'static Offender> {
    LIST.iter().find(|o| !o.model.is_empty() && model.to_lowercase().contains(&o.model.to_lowercase()) && o.firmware.eq_ignore_ascii_case(firmware.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_entries_and_every_entry_has_sources() {
        assert_eq!(for_process("AtiEClxx.exe").map(|o| o.title), Some("AMD External Events Utility (Radeon driver)"));
        assert!(for_process("explorer.exe").is_none());
        assert!(for_drive("Samsung SSD 990 PRO 2TB", "0B2QJXD7").is_some());
        assert!(for_drive("Samsung SSD 990 PRO 2TB", "4B2QJXD7").is_none(), "fixed firmware");
        assert!(for_drive("WD PC SN8000S", "0B2QJXD7").is_none());
        for o in LIST {
            assert!(!o.sources.is_empty() && o.sources.iter().all(|s| s.starts_with("https://")), "{}", o.title);
            assert!(o.process.is_empty() != o.model.is_empty(), "{}: a process or a drive", o.title);
        }
    }
}
