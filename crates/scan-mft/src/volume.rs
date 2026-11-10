//! Raw volume access: open `\\.\C:`, ask NTFS for its layout, and locate
//! every fragment of the MFT on disk.
//!
//! The volume is opened ONCE, in overlapped (asynchronous) mode. Opening a
//! raw volume is expensive (measured: time grew linearly with one handle
//! per thread), and a synchronous handle would serialize all reads. With one
//! overlapped handle, every thread reads in parallel through the same handle.

use std::alloc::{self, Layout};
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::ptr::{NonNull, null_mut};

use crate::record::{ATTR_ATTRIBUTE_LIST, ATTR_DATA, apply_fixup, attrs, data_extension_records, decode_runs, header};

const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
const ERROR_IO_PENDING: i32 = 997;

#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: *mut c_void,
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn ReadFile(file: *mut c_void, buf: *mut c_void, len: u32, read: *mut u32, overlapped: *mut Overlapped) -> i32;
    fn GetOverlappedResult(file: *mut c_void, overlapped: *mut Overlapped, transferred: *mut u32, wait: i32) -> i32;
    fn CreateEventW(security: *mut c_void, manual_reset: i32, initial_state: i32, name: *const u16) -> *mut c_void;
}

/// The parts of NTFS_VOLUME_DATA_BUFFER we need.
#[derive(Clone, Copy, Debug)]
pub struct VolumeInfo {
    pub bytes_per_sector: u64,
    pub bytes_per_cluster: u64,
    pub record_size: u64,
    pub mft_valid_len: u64,
    pub mft_start_lcn: u64,
}

/// A contiguous piece of the MFT: `clusters` clusters starting at MFT
/// cluster `vcn`, stored at volume cluster `lcn`.
#[derive(Clone, Copy, Debug)]
pub struct Extent {
    pub vcn: u64,
    pub lcn: u64,
    pub clusters: u64,
}

/// An open raw volume (overlapped, unbuffered). Shareable across threads.
pub struct Volume {
    file: File,
}

/// Per-thread reader: owns the event used to wait for its own reads.
pub struct Reader<'a> {
    vol: &'a Volume,
    event: OwnedHandle,
}

fn new_event() -> io::Result<OwnedHandle> {
    // SAFETY: plain manual-reset, initially unsignaled, unnamed event.
    let h = unsafe { CreateEventW(null_mut(), 1, 0, std::ptr::null()) };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateEventW succeeded; we own the handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(h) })
}

impl Volume {
    /// Opens a volume for raw reading. Needs administrator rights.
    ///
    /// Unbuffered: data goes straight from disk into our (4 KB-aligned,
    /// sector-multiple) buffers, skipping a copy through the system cache.
    pub fn open(letter: char) -> io::Result<Self> {
        use std::os::windows::fs::OpenOptionsExt;
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_NO_BUFFERING | FILE_FLAG_OVERLAPPED)
            .open(format!(r"\\.\{letter}:"))?;
        Ok(Self { file })
    }

    pub fn reader(&self) -> io::Result<Reader<'_>> {
        Ok(Reader { vol: self, event: new_event()? })
    }
}

impl Reader<'_> {
    /// Waits for an overlapped operation started by `start`.
    fn complete(&self, ok: i32, ov: &mut Overlapped) -> io::Result<u32> {
        if ok == 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() != Some(ERROR_IO_PENDING) {
                return Err(e);
            }
        }
        let mut n = 0u32;
        // SAFETY: `ov` is the OVERLAPPED of an operation on this handle and
        // stays alive (borrowed) until this blocking wait returns.
        if unsafe { GetOverlappedResult(self.vol.file.as_raw_handle(), ov, &mut n, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(n)
    }

    fn overlapped(&self, offset: u64) -> Overlapped {
        Overlapped {
            internal: 0,
            internal_high: 0,
            offset: offset as u32,
            offset_high: (offset >> 32) as u32,
            event: self.event.as_raw_handle(),
        }
    }

    /// Reads exactly `buf.len()` bytes at `offset`. `buf` and `offset` must
    /// be sector-aligned (unbuffered I/O).
    pub fn read_exact_at(&self, buf: &mut [u8], offset: u64) -> io::Result<()> {
        let mut done = 0;
        while done < buf.len() {
            let len = u32::try_from(buf.len() - done).unwrap_or(0xFFFF_0000); // sector-aligned cap
            let mut ov = self.overlapped(offset + done as u64);
            // SAFETY: buffer slice is valid for `len` bytes and outlives the
            // wait in `complete`.
            let ok = unsafe {
                ReadFile(self.vol.file.as_raw_handle(), buf[done..].as_mut_ptr().cast(), len, null_mut(), &mut ov)
            };
            let n = self.complete(ok, &mut ov)?;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "volume read past end"));
            }
            done += n as usize;
        }
        Ok(())
    }

    /// Reads the volume layout from the NTFS boot sector. `mft_valid_len`
    /// is filled in later by [`mft_extents`]. Measured: ~5 ms. (The first
    /// raw read of the MFT itself then takes ~0.35 s on this laptop, most
    /// likely NTFS flushing its cached MFT so raw reads see current data.)
    pub fn boot_info(&self) -> io::Result<VolumeInfo> {
        // 4 KB covers the boot sector on both 512-byte and 4K-sector disks.
        let mut buf = AlignedBuf::new(4096);
        self.read_exact_at(buf.as_mut_slice(), 0)?;
        parse_boot_sector(buf.as_mut_slice())
    }
}

/// Parses an NTFS boot sector (BIOS parameter block).
pub fn parse_boot_sector(b: &[u8]) -> io::Result<VolumeInfo> {
    let unsupported = |msg: String| io::Error::new(io::ErrorKind::Unsupported, msg);
    if b.len() < 512 || &b[3..11] != b"NTFS    " {
        return Err(unsupported("drive is not NTFS".into()));
    }
    let bytes_per_sector = u64::from(u16::from_le_bytes([b[0x0B], b[0x0C]]));
    // Sectors per cluster; values above 0x80 encode 2^(256 - v) (big clusters).
    let spc = b[0x0D];
    let sectors_per_cluster = if spc <= 0x80 { u64::from(spc) } else { 1u64 << (256 - u32::from(spc)).min(31) };
    let bytes_per_cluster = bytes_per_sector * sectors_per_cluster;
    let mft_start_lcn = u64::from_le_bytes(b[0x30..0x38].try_into().expect("8 bytes"));
    // Clusters per MFT record; negative means 2^(-v) bytes.
    let cpr = b[0x40] as i8;
    let record_size = if cpr > 0 {
        u64::from(cpr as u8) * bytes_per_cluster
    } else {
        1u64 << u32::from(cpr.unsigned_abs()).min(31)
    };
    let info = VolumeInfo { bytes_per_sector, bytes_per_cluster, record_size, mft_valid_len: 0, mft_start_lcn };
    let sane = info.bytes_per_sector >= 512
        && info.bytes_per_sector.is_power_of_two()
        && info.bytes_per_cluster.is_power_of_two()
        && info.record_size.is_power_of_two()
        && info.record_size >= 1024
        && info.bytes_per_cluster >= info.record_size
        && info.bytes_per_cluster >= info.bytes_per_sector;
    if !sane {
        return Err(unsupported(format!("unsupported NTFS layout: {info:?}")));
    }
    Ok(info)
}

/// Heap buffer aligned to 4 KB, as raw volume reads may require.
pub struct AlignedBuf {
    ptr: NonNull<u8>,
    len: usize,
}

impl AlignedBuf {
    const ALIGN: usize = 4096;

    pub fn new(len: usize) -> Self {
        let layout = Layout::from_size_align(len.max(1), Self::ALIGN).expect("valid layout");
        // SAFETY: layout has non-zero size.
        let raw = unsafe { alloc::alloc_zeroed(layout) };
        let ptr = NonNull::new(raw).unwrap_or_else(|| alloc::handle_alloc_error(layout));
        Self { ptr, len }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY: we own `len` initialized (zeroed) bytes at `ptr`.
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.len) }
    }
}

impl Drop for AlignedBuf {
    fn drop(&mut self) {
        let layout = Layout::from_size_align(self.len.max(1), Self::ALIGN).expect("valid layout");
        // SAFETY: allocated in `new` with this exact layout.
        unsafe { alloc::dealloc(self.ptr.as_ptr(), layout) }
    }
}

// SAFETY: AlignedBuf uniquely owns its allocation, like a Vec<u8>.
unsafe impl Send for AlignedBuf {}

/// Reads one MFT record by number, using the extents known so far.
fn read_record(vol: &Reader, info: &VolumeInfo, extents: &[Extent], rec_no: u64) -> io::Result<Vec<u8>> {
    let mft_off = rec_no * info.record_size;
    let vcn = mft_off / info.bytes_per_cluster;
    let e = extents
        .iter()
        .find(|e| vcn >= e.vcn && vcn < e.vcn + e.clusters)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, format!("MFT record {rec_no} not mapped")))?;
    let disk = (e.lcn + (vcn - e.vcn)) * info.bytes_per_cluster + mft_off % info.bytes_per_cluster;
    let aligned = disk / info.bytes_per_sector * info.bytes_per_sector;
    let skip = (disk - aligned) as usize;
    let len = (skip as u64 + info.record_size).next_multiple_of(info.bytes_per_sector) as usize;
    let mut buf = AlignedBuf::new(len);
    vol.read_exact_at(buf.as_mut_slice(), aligned)?;
    let mut rec = buf.as_mut_slice()[skip..skip + info.record_size as usize].to_vec();
    if !apply_fixup(&mut rec) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("MFT record {rec_no} is corrupt")));
    }
    Ok(rec)
}

/// Adds the runs of one unnamed, non-resident $DATA attribute of `rec`.
/// Returns the stream's valid (initialized) length if this is its first
/// extent.
fn push_data_runs(rec: &[u8], extents: &mut Vec<Extent>) -> io::Result<Option<u64>> {
    let bad = |msg: &str| io::Error::new(io::ErrorKind::InvalidData, msg.to_owned());
    let h = header(rec).ok_or_else(|| bad("bad MFT record header"))?;
    let mut valid_len = None;
    for a in attrs(rec, &h) {
        if a.typ != ATTR_DATA || !a.is_unnamed() || !a.non_resident() {
            continue;
        }
        let mut vcn = a.lowest_vcn().ok_or_else(|| bad("bad $MFT data attribute"))?;
        if vcn == 0 {
            valid_len = a.initialized_size();
        }
        let runs = a.runlist().and_then(decode_runs).ok_or_else(|| bad("bad $MFT run list"))?;
        for r in runs {
            let lcn = r.lcn.ok_or_else(|| bad("sparse run inside $MFT"))?;
            extents.push(Extent { vcn, lcn, clusters: r.len });
            vcn += r.len;
        }
    }
    Ok(valid_len)
}

/// Finds every fragment of the MFT, including fragments listed in
/// extension records (a heavily fragmented MFT has an $ATTRIBUTE_LIST).
/// Also sets `info.mft_valid_len`.
pub fn mft_extents(vol: &Reader, info: &mut VolumeInfo) -> io::Result<Vec<Extent>> {
    let first = vec![Extent { vcn: 0, lcn: info.mft_start_lcn, clusters: 1 }];
    let rec0 = read_record(vol, info, &first, 0)?;
    let mut extents = Vec::new();
    info.mft_valid_len = push_data_runs(&rec0, &mut extents)?
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "$MFT has no data stream"))?;

    let h = header(&rec0).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "bad $MFT header"))?;
    let mut extension_recs = Vec::new();
    for a in attrs(&rec0, &h) {
        if a.typ == ATTR_ATTRIBUTE_LIST {
            match a.value() {
                Some(list) => extension_recs = data_extension_records(list),
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::Unsupported,
                        "MFT attribute list is non-resident (extremely fragmented MFT)",
                    ));
                }
            }
        }
    }
    for rec_no in extension_recs {
        let rec = read_record(vol, info, &extents, rec_no)?;
        if let Some(len) = push_data_runs(&rec, &mut extents)? {
            info.mft_valid_len = len;
        }
    }
    extents.sort_by_key(|e| e.vcn);

    let mapped: u64 = extents.iter().map(|e| e.clusters).sum::<u64>() * info.bytes_per_cluster;
    if mapped < info.mft_valid_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("MFT runs cover {mapped} bytes, need {}", info.mft_valid_len),
        ));
    }
    Ok(extents)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boot(bytes_per_sector: u16, spc: u8, mft_lcn: u64, cpr: i8) -> Vec<u8> {
        let mut b = vec![0u8; 512];
        b[3..11].copy_from_slice(b"NTFS    ");
        b[0x0B..0x0D].copy_from_slice(&bytes_per_sector.to_le_bytes());
        b[0x0D] = spc;
        b[0x30..0x38].copy_from_slice(&mft_lcn.to_le_bytes());
        b[0x40] = cpr as u8;
        b
    }

    #[test]
    fn parses_boot_sectors() {
        // Typical: 512-byte sectors, 4 KB clusters, 1 KB records (-10 => 2^10).
        let i = parse_boot_sector(&boot(512, 8, 786_432, -10)).unwrap();
        assert_eq!((i.bytes_per_sector, i.bytes_per_cluster, i.record_size, i.mft_start_lcn), (512, 4096, 1024, 786_432));
        // 4K-native disk with 4 KB records.
        let i = parse_boot_sector(&boot(4096, 1, 3, -12)).unwrap();
        assert_eq!((i.bytes_per_cluster, i.record_size), (4096, 4096));
        // 2 MB clusters use the 2^(256 - v) encoding: 0xF4 => 2^12 sectors.
        let i = parse_boot_sector(&boot(512, 0xF4, 3, -10)).unwrap();
        assert_eq!(i.bytes_per_cluster, 2 << 20);

        let mut fat = boot(512, 8, 3, -10);
        fat[3..11].copy_from_slice(b"MSDOS5.0");
        assert_eq!(parse_boot_sector(&fat).unwrap_err().kind(), io::ErrorKind::Unsupported);
        // Record bigger than a cluster is rejected (we read whole clusters).
        assert!(parse_boot_sector(&boot(512, 1, 3, -10)).is_err());
    }
}
