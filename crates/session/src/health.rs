//! Drive health: what wears the drive, not only what fills it.
//!
//! Reads, without admin and without even read access to the disk:
//! - which physical disk holds each drive letter (`IOCTL_STORAGE_GET_DEVICE_NUMBER`),
//! - model, firmware and bus (`StorageDeviceProperty`),
//! - SSD or spinning disk (`StorageDeviceSeekPenaltyProperty`), TRIM support,
//! - for NVMe drives, the standard SMART / health log (log page 2) through
//!   `StorageDeviceProtocolSpecificProperty`: % of rated life used, data
//!   written and read, temperature, spare, errors, power-on hours.
//!
//! Measured on Windows 11 (xylem-rnd E14): the disk is opened with no access
//! rights at all (opening it for reading needs admin). Everything here only
//! asks the drive for reports; nothing is written. SATA drives' SMART needs
//! admin (ATA pass-through) and comes later through the helper.

use std::ffi::c_void;

use serde::{Deserialize, Serialize};

const IOCTL_STORAGE_QUERY_PROPERTY: u32 = 0x002D_1400;
const IOCTL_STORAGE_GET_DEVICE_NUMBER: u32 = 0x002D_1080;
/// NVMe "data unit" = 1000 sectors of 512 bytes.
const DATA_UNIT: u128 = 512_000;

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, tmpl: *mut c_void) -> *mut c_void;
    fn DeviceIoControl(h: *mut c_void, code: u32, inb: *const c_void, inl: u32, outb: *mut c_void, outl: u32, ret: *mut u32, ov: *mut c_void) -> i32;
    fn CloseHandle(h: *mut c_void) -> i32;
    fn GetLastError() -> u32;
}
#[link(name = "advapi32", kind = "raw-dylib")]
unsafe extern "system" {
    fn RegGetValueW(key: *mut c_void, sub: *const u16, value: *const u16, flags: u32, kind: *mut u32, data: *mut c_void, bytes: *mut u32) -> i32;
}

/// A handle opened with **no access rights**: enough for storage queries,
/// can't read or write a single sector.
struct Dev(*mut c_void);

impl Dev {
    fn open(path: &str) -> Option<Self> {
        let w: Vec<u16> = path.encode_utf16().chain([0]).collect();
        // SAFETY: NUL-terminated path; access 0; share read+write; OPEN_EXISTING.
        let h = unsafe { CreateFileW(w.as_ptr(), 0, 3, std::ptr::null_mut(), 3, 0, std::ptr::null_mut()) };
        (h as isize != -1 && !h.is_null()).then_some(Self(h))
    }

    fn ioctl(&self, code: u32, input: &[u8], out_len: usize) -> Result<Vec<u8>, u32> {
        let mut out = vec![0u8; out_len];
        let mut got = 0u32;
        // SAFETY: buffers are valid for the lengths given; synchronous call.
        let ok = unsafe { DeviceIoControl(self.0, code, input.as_ptr().cast(), input.len() as u32, out.as_mut_ptr().cast(), out.len() as u32, &mut got, std::ptr::null_mut()) };
        if ok == 0 {
            return Err(unsafe { GetLastError() });
        }
        out.truncate(got as usize);
        Ok(out)
    }

    /// `IOCTL_STORAGE_QUERY_PROPERTY`, standard query for `property`.
    fn property(&self, property: u32, out_len: usize) -> Result<Vec<u8>, u32> {
        let mut q = [0u8; 12];
        q[..4].copy_from_slice(&property.to_le_bytes());
        self.ioctl(IOCTL_STORAGE_QUERY_PROPERTY, &q, out_len)
    }
}

impl Drop for Dev {
    fn drop(&mut self) {
        // SAFETY: a handle we opened.
        unsafe { CloseHandle(self.0) };
    }
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// The NVMe SMART / health information log (512 bytes, NVMe spec 5.14.1.2).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct NvmeHealth {
    /// Bits: 0 spare below threshold, 1 temperature, 2 reliability degraded,
    /// 3 read-only, 4 volatile backup failed, 5 persistent memory read-only.
    pub critical_warning: u8,
    pub temp_c: i32,
    /// Spare blocks left, % (and the level the maker calls critical).
    pub spare: u8,
    pub spare_threshold: u8,
    /// The drive's own wear estimate: 100 = rated life used (may exceed 100).
    pub used_pct: u8,
    pub read_bytes: u128,
    pub written_bytes: u128,
    pub power_on_hours: u128,
    pub power_cycles: u128,
    pub unsafe_shutdowns: u128,
    pub media_errors: u128,
    pub error_log_entries: u128,
    /// Minutes spent above the warning / critical temperature.
    pub warn_temp_minutes: u32,
    pub crit_temp_minutes: u32,
}

impl NvmeHealth {
    /// Parses the 512-byte log. `None` if it's too short.
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < 512 {
            return None;
        }
        let u128_at = |o: usize| u128::from_le_bytes(b[o..o + 16].try_into().expect("16 bytes"));
        let kelvin = i32::from(u16::from_le_bytes([b[1], b[2]]));
        Some(Self {
            critical_warning: b[0],
            temp_c: if kelvin == 0 { 0 } else { kelvin - 273 },
            spare: b[3],
            spare_threshold: b[4],
            used_pct: b[5],
            read_bytes: u128_at(32).saturating_mul(DATA_UNIT),
            written_bytes: u128_at(48).saturating_mul(DATA_UNIT),
            power_on_hours: u128_at(128),
            power_cycles: u128_at(112),
            unsafe_shutdowns: u128_at(144),
            media_errors: u128_at(160),
            error_log_entries: u128_at(176),
            warn_temp_minutes: u32::from_le_bytes(b[192..196].try_into().expect("4 bytes")),
            crit_temp_minutes: u32::from_le_bytes(b[196..200].try_into().expect("4 bytes")),
        })
    }

    /// The critical-warning bits as sentences.
    pub fn warnings(&self) -> Vec<&'static str> {
        let w = self.critical_warning;
        [
            (0x01, "Spare blocks are below the safe level."),
            (0x02, "The temperature is outside the safe range."),
            (0x04, "The drive reports its reliability is degraded (too many errors)."),
            (0x08, "The drive has switched itself to read-only."),
            (0x10, "The drive's power-loss backup failed."),
            (0x20, "Part of the drive's memory became read-only."),
        ]
        .into_iter()
        .filter(|&(bit, _)| w & bit != 0)
        .map(|(_, t)| t)
        .collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Good,
    /// Fine for now, worth an eye.
    Watch,
    /// Back up and act.
    Bad,
    Unknown,
}

#[derive(Clone, Debug, Serialize)]
pub struct Drive {
    pub number: u32,
    /// Drive letters on this disk, e.g. ["C:", "D:"].
    pub letters: Vec<String>,
    pub model: String,
    pub firmware: String,
    /// "NVMe", "SATA", "USB", ...
    pub bus: String,
    /// `Some(true)` SSD, `Some(false)` spinning disk, `None` unknown.
    pub ssd: Option<bool>,
    /// The device supports TRIM.
    pub trim: Option<bool>,
    pub nvme: Option<NvmeHealth>,
    /// Why there's no health report, in plain English.
    pub note: Option<String>,
}

fn bus_name(t: u32) -> &'static str {
    match t {
        1 => "SCSI",
        2 => "ATAPI",
        3 => "ATA",
        4 => "FireWire",
        6 => "Fibre Channel",
        7 => "USB",
        8 => "RAID",
        10 => "SAS",
        11 => "SATA",
        12 => "SD card",
        13 => "MMC",
        14 => "Virtual",
        15 => "Virtual disk (VHD)",
        17 => "NVMe",
        18 => "SCM",
        19 => "UFS",
        _ => "other",
    }
}

fn c_string(b: &[u8], off: u32) -> String {
    let off = off as usize;
    if off == 0 || off >= b.len() {
        return String::new();
    }
    let s = &b[off..];
    String::from_utf8_lossy(&s[..s.iter().position(|&c| c == 0).unwrap_or(s.len())]).trim().to_owned()
}

/// Asks the NVMe drive for its health log (page 2, controller-wide).
fn nvme_log(dev: &Dev) -> Result<NvmeHealth, u32> {
    // STORAGE_PROPERTY_QUERY (PropertyId 50 = StorageDeviceProtocolSpecificProperty,
    // QueryType 0) followed by STORAGE_PROTOCOL_SPECIFIC_DATA (ProtocolType 3 =
    // NVMe, DataType 2 = log page, request 2 = health, sub-value 0, data at
    // offset 40, 512 bytes).
    let mut q = vec![0u8; 8 + 40 + 512];
    for (o, v) in [(0, 50u32), (8, 3), (12, 2), (16, 2), (24, 40), (28, 512)] {
        q[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    let out = dev.ioctl(IOCTL_STORAGE_QUERY_PROPERTY, &q, q.len())?;
    // STORAGE_PROTOCOL_DATA_DESCRIPTOR: Version, Size, then the 40-byte
    // specific data, whose offset/length fields say where the log is.
    let off = u32_at(&out, 8 + 16).ok_or(13u32)? as usize;
    let len = u32_at(&out, 8 + 20).ok_or(13u32)? as usize;
    let log = out.get(8 + off..(8 + off).saturating_add(len)).ok_or(13u32)?;
    NvmeHealth::parse(log).ok_or(13)
}

fn describe(number: u32, letters: Vec<String>) -> Option<Drive> {
    let dev = Dev::open(&format!(r"\\.\PhysicalDrive{number}"))?;
    let desc = dev.property(0, 4096).ok()?;
    let bus_type = u32_at(&desc, 28).unwrap_or(0);
    let bus = bus_name(bus_type).to_owned();
    // DEVICE_SEEK_PENALTY_DESCRIPTOR / DEVICE_TRIM_DESCRIPTOR: Version, Size, BOOLEAN.
    let ssd = dev.property(7, 12).ok().and_then(|b| b.get(8).map(|&p| p == 0));
    let trim = dev.property(8, 12).ok().and_then(|b| b.get(8).map(|&t| t != 0));
    let (nvme, note) = if bus_type == 17 {
        match nvme_log(&dev) {
            Ok(h) => (Some(h), None),
            Err(e) => (None, Some(format!("The drive didn't give its health report (Windows error {e})."))),
        }
    } else if ssd == Some(false) {
        (None, Some("A spinning hard disk: SSD wear doesn't apply. Its SMART report needs admin; Xylem will read it in a later version.".into()))
    } else {
        (None, Some(format!("A {bus} drive: its health report (SMART) needs admin. Xylem will read it in a later version.")))
    };
    Some(Drive { number, letters, model: c_string(&desc, u32_at(&desc, 16).unwrap_or(0)), firmware: c_string(&desc, u32_at(&desc, 20).unwrap_or(0)), bus, ssd, trim, nvme, note })
}

/// Physical disk number holding a drive letter ("C:\\" -> 0).
pub fn disk_of(letter: &str) -> Option<u32> {
    let l = letter.get(..1)?;
    let dev = Dev::open(&format!(r"\\.\{l}:"))?;
    // STORAGE_DEVICE_NUMBER: DeviceType, DeviceNumber, PartitionNumber.
    u32_at(&dev.ioctl(IOCTL_STORAGE_GET_DEVICE_NUMBER, &[], 12).ok()?, 4)
}

/// Bytes written to physical disk `n` since boot (`IOCTL_DISK_PERFORMANCE`,
/// no access rights needed).
pub fn disk_written(n: u32) -> Option<u64> {
    const IOCTL_DISK_PERFORMANCE: u32 = 0x0007_0020;
    let dev = Dev::open(&format!(r"\\.\PhysicalDrive{n}"))?;
    // DISK_PERFORMANCE: BytesRead (i64), BytesWritten (i64), ...
    let out = dev.ioctl(IOCTL_DISK_PERFORMANCE, &[], 128).ok()?;
    let w = i64::from_le_bytes(out.get(8..16)?.try_into().ok()?);
    u64::try_from(w).ok()
}

/// Every physical disk Windows lists, with its letters and health report.
pub fn drives() -> Vec<Drive> {
    let mut by_disk: std::collections::BTreeMap<u32, Vec<String>> = std::collections::BTreeMap::new();
    for d in crate::drives() {
        if let Some(n) = disk_of(&d) {
            by_disk.entry(n).or_default().push(d.trim_end_matches('\\').to_owned());
        }
    }
    // Disks without letters too (numbers can have gaps).
    for n in 0..32 {
        by_disk.entry(n).or_default();
    }
    by_disk.into_iter().filter_map(|(n, letters)| describe(n, letters)).collect()
}

/// Whether the drive holding `letter` is an SSD (for the nearly-full hint).
pub fn is_ssd(letter: &str) -> Option<bool> {
    let n = disk_of(letter)?;
    let dev = Dev::open(&format!(r"\\.\PhysicalDrive{n}"))?;
    dev.property(7, 12).ok().and_then(|b| b.get(8).map(|&p| p == 0))
}

/// Windows sends TRIM to SSDs (`DisableDeleteNotification` = 0). `None` if
/// the value can't be read.
pub fn trim_enabled() -> Option<bool> {
    // HKEY_LOCAL_MACHINE is the sign-extended constant 0x80000002.
    let hklm = std::ptr::without_provenance_mut::<c_void>(0x8000_0002_u32 as i32 as isize as usize);
    const RRF_RT_REG_DWORD: u32 = 0x10;
    let sub: Vec<u16> = r"SYSTEM\CurrentControlSet\Control\FileSystem".encode_utf16().chain([0]).collect();
    let val: Vec<u16> = "DisableDeleteNotification".encode_utf16().chain([0]).collect();
    let mut data = 0u32;
    let mut len = 4u32;
    // SAFETY: NUL-terminated strings, a 4-byte buffer for a DWORD.
    let r = unsafe { RegGetValueW(hklm, sub.as_ptr(), val.as_ptr(), RRF_RT_REG_DWORD, std::ptr::null_mut(), (&raw mut data).cast(), &mut len) };
    (r == 0).then_some(data == 0)
}

fn tb(b: u128) -> String {
    xylem_core::format::bytes(u64::try_from(b).unwrap_or(u64::MAX))
}

fn hours_text(h: u128) -> String {
    let d = h / 24;
    match h {
        0..=47 => format!("{h} powered-on hours"),
        _ if d < 60 => format!("{} powered-on hours (about {d} days)", xylem_core::format::count(h as u64)),
        _ if d < 730 => format!("{} powered-on hours (about {} months)", xylem_core::format::count(h as u64), d / 30),
        _ => format!("{} powered-on hours (about {:.1} years)", xylem_core::format::count(h as u64), d as f64 / 365.0),
    }
}

impl Drive {
    /// Overall verdict and one plain sentence.
    pub fn verdict(&self) -> (Level, String) {
        let Some(h) = &self.nvme else {
            return (Level::Unknown, self.note.clone().unwrap_or_else(|| "No health report.".into()));
        };
        let warnings = h.warnings();
        if !warnings.is_empty() || h.media_errors > 0 || (h.spare_threshold > 0 && h.spare < h.spare_threshold) || h.used_pct >= 100 {
            let why = if let Some(w) = warnings.first() {
                (*w).to_owned()
            } else if h.media_errors > 0 {
                format!("The drive has recorded {} media errors (data it couldn't read back).", h.media_errors)
            } else if h.used_pct >= 100 {
                format!("{}% of its rated life is used. It may keep working, but it's past what the maker promises.", h.used_pct)
            } else {
                "Spare blocks are below the safe level.".into()
            };
            return (Level::Bad, format!("Back up now. {why}"));
        }
        if h.used_pct >= 80 || (h.spare_threshold > 0 && h.spare < h.spare_threshold.saturating_add(10)) || h.crit_temp_minutes > 0 {
            let why = if h.used_pct >= 80 {
                format!("{}% of its rated life is used.", h.used_pct)
            } else if h.crit_temp_minutes > 0 {
                format!("It has run above its critical temperature for {} minutes.", h.crit_temp_minutes)
            } else {
                format!("Spare blocks are getting low ({}%).", h.spare)
            };
            return (Level::Watch, format!("Keep an eye on it. {why} Keep backups current."));
        }
        (Level::Good, format!("Good: {}% of rated life used.", h.used_pct))
    }

    /// The details as short plain lines.
    pub fn facts(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(h) = &self.nvme {
            out.push(format!("{} written · {} read · {}", tb(h.written_bytes), tb(h.read_bytes), hours_text(h.power_on_hours)));
            let mut t = format!("{} °C now", h.temp_c);
            if h.warn_temp_minutes > 0 {
                t += &format!(" · ran hot (above its warning temperature) for {} min in total", xylem_core::format::count(u64::from(h.warn_temp_minutes)));
            }
            out.push(t);
            out.push(format!("Spare blocks {}% (critical at {}%) · {} media errors", h.spare, h.spare_threshold, h.media_errors));
            out.push(format!(
                "{} power cycles · {} unsafe shutdowns (power cut or held power button)",
                xylem_core::format::count(u64::try_from(h.power_cycles).unwrap_or(u64::MAX)),
                xylem_core::format::count(u64::try_from(h.unsafe_shutdowns).unwrap_or(u64::MAX))
            ));
            for w in h.warnings() {
                out.push(w.to_owned());
            }
        }
        if let Some(t) = self.trim
            && self.ssd == Some(true)
        {
            out.push(if t { "Supports TRIM (lets the drive erase freed space early: faster, less wear)".into() } else { "Doesn't report TRIM support".into() });
        }
        out
    }
}

/// "C: is 92% full. SSDs slow down and wear faster when nearly full: aim
/// for at least 10-15% free." `None` when there's enough room.
pub fn nearly_full(letter: &str, total: u64, free: u64, ssd: bool) -> Option<String> {
    if !ssd || total == 0 || free.saturating_mul(10) >= total {
        return None;
    }
    let used = 100 - free.saturating_mul(100) / total;
    Some(format!("{} is {used}% full. SSDs get slower and wear faster when nearly full: aim for at least 10-15% free.", letter.trim_end_matches('\\')))
}

// ---------------------------------------------------------------------------
// Daily snapshots and trends (writes per day, life estimate)
// ---------------------------------------------------------------------------

const DAY: u64 = 86_400;
/// Keep at most this many snapshot lines (a few drives for years).
const KEEP: usize = 4000;

/// One day's numbers for one drive, in `%LOCALAPPDATA%\Xylem\health.jsonl`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Snap {
    pub time: u64,
    /// Which drive: model and disk number ("WD PC SN8000S...#0").
    pub drive: String,
    pub written: u64,
    pub read: u64,
    pub used_pct: u8,
    pub hours: u64,
    pub temp_c: i32,
    pub media_errors: u64,
}

fn drive_key(d: &Drive) -> String {
    format!("{}#{}", d.model, d.number)
}

fn snaps_path() -> Option<std::path::PathBuf> {
    Some(std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Xylem").join("health.jsonl"))
}

/// All saved snapshots, oldest first (damaged lines are skipped).
pub fn snaps() -> Vec<Snap> {
    let Some(text) = snaps_path().and_then(|p| std::fs::read_to_string(p).ok()) else { return Vec::new() };
    text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()
}

/// Adds today's numbers for the NVMe drives in `drives` (one line per drive
/// per day: a newer reading the same day replaces the earlier one).
pub fn merge(mut all: Vec<Snap>, drives: &[Drive], now: u64) -> Vec<Snap> {
    for d in drives {
        let Some(h) = &d.nvme else { continue };
        let s = Snap {
            time: now,
            drive: drive_key(d),
            written: u64::try_from(h.written_bytes).unwrap_or(u64::MAX),
            read: u64::try_from(h.read_bytes).unwrap_or(u64::MAX),
            used_pct: h.used_pct,
            hours: u64::try_from(h.power_on_hours).unwrap_or(u64::MAX),
            temp_c: h.temp_c,
            media_errors: u64::try_from(h.media_errors).unwrap_or(u64::MAX),
        };
        match all.iter_mut().rev().find(|x| x.drive == s.drive) {
            Some(last) if last.time / DAY == now / DAY => *last = s,
            _ => all.push(s),
        }
    }
    if all.len() > KEEP {
        all.drain(..all.len() - KEEP);
    }
    all
}

/// Saves today's snapshot of `drives`. Called by the app and by scheduled
/// scans; cheap (one small file).
pub fn record_daily(drives: &[Drive]) {
    let Some(p) = snaps_path() else { return };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let all = merge(snaps(), drives, now);
    let text: String = all.iter().filter_map(|s| serde_json::to_string(s).ok()).map(|l| l + "\n").collect();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// How a drive is doing over time.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Trend {
    /// Days covered by all of this drive's snapshots.
    pub days: f64,
    /// Average bytes written per day over the last 30 days (None: under 2 days of data).
    pub per_day: Option<f64>,
    /// (time, average bytes written per day since the snapshot before).
    pub daily: Vec<(u64, u64)>,
    /// Plain-English life estimate.
    pub life: String,
}

/// The trend of one drive from its snapshots (oldest first).
pub fn trend(all: &[Snap], d: &Drive) -> Option<Trend> {
    let key = drive_key(d);
    let mine: Vec<&Snap> = all.iter().filter(|s| s.drive == key).collect();
    let last = *mine.last()?;
    // Written counters only go up; a lower value means a different drive or a reset.
    let daily: Vec<(u64, u64)> = mine
        .windows(2)
        .filter(|w| w[1].time > w[0].time && w[1].written >= w[0].written)
        .map(|w| (w[1].time, ((w[1].written - w[0].written) as f64 / ((w[1].time - w[0].time) as f64 / DAY as f64).max(1.0 / 24.0)) as u64))
        .collect();
    let first: &Snap = mine.iter().find(|s| s.time.saturating_add(30 * DAY) >= last.time).unwrap_or(&mine[0]);
    let span = last.time.saturating_sub(first.time) as f64 / DAY as f64;
    let per_day = (span >= 2.0 && last.written >= first.written).then(|| (last.written - first.written) as f64 / span);
    let all_span = last.time.saturating_sub(mine[0].time) as f64 / DAY as f64;
    Some(Trend { days: all_span, per_day, daily, life: life_text(&mine, last) })
}

/// Rated-life estimate. The drive reports wear in whole percent, so an
/// estimate needs the number to have gone up; until then it says how long
/// it has *not* moved. Always a range, never a date.
fn life_text(mine: &[&Snap], last: &Snap) -> String {
    let left = 100u32.saturating_sub(u32::from(last.used_pct));
    if left == 0 {
        return "Rated life is used up (100% or more). The drive can keep working, but keep backups current.".into();
    }
    let years = |days: f64| days / 365.0;
    let span_of = |s: &Snap| last.time.saturating_sub(s.time) as f64 / DAY as f64;
    // Earliest snapshot within a year (and at least a week back) with a lower % used.
    if let Some(from) = mine.iter().find(|s| s.used_pct < last.used_pct && span_of(s) <= 365.0 && span_of(s) >= 7.0) {
        let span = span_of(from);
        let rose = f64::from(last.used_pct - from.used_pct);
        // Whole percents: the true rise is between rose-1 and rose+1.
        let fast = years(f64::from(left) / ((rose + 1.0) / span));
        let slow = (rose > 1.0).then(|| years(f64::from(left) / ((rose - 1.0) / span)));
        return match slow {
            _ if fast > 50.0 => "At this pace, rated life is decades away.".into(),
            Some(slow) if slow < 50.0 => format!("At the pace of the last {span:.0} days, rated life would be reached in roughly {:.0}–{slow:.0} years.", fast.max(1.0)),
            _ => format!("At the pace of the last {span:.0} days, rated life would be reached in at least {:.1} years.", fast.max(0.1)),
        };
    }
    let span = span_of(mine[0]);
    if span < 30.0 {
        let so_far = if span < 1.0 { "started today".to_owned() } else { format!("{span:.0} days so far") };
        return format!("Too early to tell: the drive reports wear in whole percent. Xylem keeps a daily note ({so_far}); after a month, or when it moves, there's an estimate.");
    }
    // Under 1% in `span` days -> more than left * span days.
    let more_than = years(f64::from(left) * span);
    if more_than > 50.0 {
        format!("Wear hasn't moved in {span:.0} days: at this pace, rated life is decades away.")
    } else {
        format!("Wear hasn't moved in {span:.0} days: at this pace, more than {more_than:.0} years of rated life are left.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A health log like the one this PC's WD SN8000S returned (E14).
    fn sample_log() -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[1..3].copy_from_slice(&305u16.to_le_bytes()); // 32 C
        b[3] = 100;
        b[4] = 10;
        b[5] = 1;
        let put = |b: &mut Vec<u8>, o: usize, v: u128| b[o..o + 16].copy_from_slice(&v.to_le_bytes());
        put(&mut b, 32, 262_363_000); // ~134 TB read
        put(&mut b, 48, 71_504_000); // ~36.6 TB written
        put(&mut b, 112, 1159);
        put(&mut b, 128, 1071);
        put(&mut b, 144, 85);
        b
    }

    #[test]
    fn parses_the_health_log() {
        let h = NvmeHealth::parse(&sample_log()).unwrap();
        assert_eq!((h.temp_c, h.spare, h.spare_threshold, h.used_pct), (32, 100, 10, 1));
        assert_eq!(h.written_bytes, 71_504_000 * 512_000);
        assert_eq!((h.power_on_hours, h.power_cycles, h.unsafe_shutdowns, h.media_errors), (1071, 1159, 85, 0));
        assert!(h.warnings().is_empty());
        assert!(NvmeHealth::parse(&[0u8; 511]).is_none());
        // Any 512 bytes parse without panicking (saturating byte counts).
        let mut rng = 0x2545_f491_4f6c_dd1d_u64;
        for _ in 0..20_000 {
            let b: Vec<u8> = (0..512)
                .map(|_| {
                    rng ^= rng << 13;
                    rng ^= rng >> 7;
                    rng ^= rng << 17;
                    rng as u8
                })
                .collect();
            let h = NvmeHealth::parse(&b).unwrap();
            let d = Drive { number: 0, letters: vec![], model: String::new(), firmware: String::new(), bus: "NVMe".into(), ssd: Some(true), trim: Some(true), nvme: Some(h), note: None };
            let _ = (d.verdict(), d.facts());
        }
    }

    fn drive(h: NvmeHealth) -> Drive {
        Drive { number: 0, letters: vec!["C:".into()], model: "X".into(), firmware: "1".into(), bus: "NVMe".into(), ssd: Some(true), trim: Some(true), nvme: Some(h), note: None }
    }

    #[test]
    fn verdicts_read_plainly() {
        let good = drive(NvmeHealth::parse(&sample_log()).unwrap());
        assert_eq!(good.verdict(), (Level::Good, "Good: 1% of rated life used.".into()));
        assert!(good.facts()[0].starts_with("33.3 TB written · 122 TB read · 1,071 powered-on hours (about 44 days)"), "{:?}", good.facts());
        let mut h = NvmeHealth::parse(&sample_log()).unwrap();
        h.used_pct = 85;
        assert_eq!(drive(h.clone()).verdict().0, Level::Watch);
        h.used_pct = 101;
        assert_eq!(drive(h.clone()).verdict().0, Level::Bad);
        h.used_pct = 5;
        h.media_errors = 3;
        let (l, t) = drive(h.clone()).verdict();
        assert_eq!(l, Level::Bad);
        assert!(t.contains("3 media errors"), "{t}");
        h.media_errors = 0;
        h.critical_warning = 0x08;
        assert!(drive(h).verdict().1.contains("read-only"));
        let none = Drive { nvme: None, note: Some("needs admin".into()), ..good };
        assert_eq!(none.verdict(), (Level::Unknown, "needs admin".into()));
    }

    #[test]
    fn nearly_full_only_for_ssds_under_10_percent() {
        const GB: u64 = 1 << 30;
        assert!(nearly_full("C:\\", 1000 * GB, 50 * GB, true).unwrap().starts_with("C: is 95% full"));
        assert!(nearly_full("C:\\", 1000 * GB, 50 * GB, false).is_none(), "not an SSD");
        assert!(nearly_full("C:\\", 1000 * GB, 150 * GB, true).is_none());
        assert!(nearly_full("C:\\", 0, 0, true).is_none());
    }

    fn snap(day: u64, written_gb: u64, used: u8) -> Snap {
        Snap { time: day * DAY + 3600, drive: "X#0".into(), written: written_gb << 30, read: 0, used_pct: used, hours: day * 8, temp_c: 35, media_errors: 0 }
    }

    #[test]
    fn one_snapshot_per_drive_per_day() {
        let mut h = NvmeHealth::parse(&sample_log()).unwrap();
        let d = drive(h.clone());
        let a = merge(Vec::new(), std::slice::from_ref(&d), 10 * DAY + 100);
        h.written_bytes += 1 << 30;
        let d2 = drive(h);
        let b = merge(a, std::slice::from_ref(&d2), 10 * DAY + 5000);
        assert_eq!(b.len(), 1, "same day replaces");
        assert_eq!(b[0].written, u64::try_from(d2.nvme.as_ref().unwrap().written_bytes).unwrap());
        let c = merge(b, std::slice::from_ref(&d2), 11 * DAY + 10);
        assert_eq!(c.len(), 2, "next day adds");
        let none = Drive { nvme: None, ..d2 };
        assert_eq!(merge(c.clone(), &[none], 12 * DAY).len(), 2, "no health report: nothing saved");
    }

    #[test]
    fn writes_per_day_and_life_estimate() {
        let d = drive(NvmeHealth::default());
        // 40 GB a day for 60 days; wear 2% -> 4% over that time.
        let all: Vec<Snap> = (0..=60).map(|day| snap(day, 1000 + 40 * day, if day < 20 { 2 } else if day < 50 { 3 } else { 4 })).collect();
        let t = trend(&all, &d).unwrap();
        assert!((t.per_day.unwrap() / (1u64 << 30) as f64 - 40.0).abs() < 0.01);
        assert_eq!(t.daily.len(), 60);
        assert!(t.life.starts_with("At the pace of the last 60 days, rated life would be reached in roughly"), "{}", t.life);
        // Too early, and "hasn't moved".
        let early: Vec<Snap> = (0..5).map(|day| snap(day, 100 + day, 1)).collect();
        assert!(trend(&early, &d).unwrap().life.starts_with("Too early to tell"));
        let flat: Vec<Snap> = (0..=40).map(|day| snap(day, 100 + day, 1)).collect();
        let l = trend(&flat, &d).unwrap().life;
        assert!(l.starts_with("Wear hasn't moved in 40 days: at this pace, more than 11 years"), "{l}");
        // Another drive's snapshots are ignored; one snapshot: no per-day yet.
        assert!(trend(&all, &Drive { number: 1, ..d.clone() }).is_none());
        assert_eq!(trend(&all[..1], &d).unwrap().per_day, None);
        // A counter that goes down (drive swapped) doesn't make negative writes.
        let swapped = vec![snap(0, 500, 1), snap(1, 10, 1), snap(2, 20, 1), snap(3, 30, 1)];
        assert!(trend(&swapped, &d).unwrap().daily.iter().all(|x| x.1 < 20 << 30));
    }

    /// Reads this PC's drives (read-only queries). cargo test -- --ignored real_drives --nocapture
    #[test]
    #[ignore = "reads the real drives"]
    fn real_drives() {
        let ds = drives();
        assert!(!ds.is_empty());
        for d in &ds {
            eprintln!("disk {} {:?} {} {} {} ssd={:?} trim={:?}: {:?}", d.number, d.letters, d.model, d.firmware, d.bus, d.ssd, d.trim, d.verdict());
            for f in d.facts() {
                eprintln!("    {f}");
            }
        }
        eprintln!("TRIM on in Windows: {:?}", trim_enabled());
        assert!(ds.iter().any(|d| d.letters.iter().any(|l| l == "C:")));
    }
}
