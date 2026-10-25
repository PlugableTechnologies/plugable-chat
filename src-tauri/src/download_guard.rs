//! Checks made before a large model download starts, so a full disk is reported in plain words
//! up front instead of as a cryptic failure part-way through a multi-gigabyte download.

use std::path::Path;

/// Extra room beyond the download itself: the archive is unpacked next to the finished files.
const UNPACK_HEADROOM_PERCENT: u64 = 20;
/// Never start a download that would leave the drive with less than this free.
const MIN_FREE_AFTER_MB: u64 = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiskVerdict {
    Enough,
    /// Free space or model size could not be read; do not block the download on a guess.
    Unknown,
    NotEnough { needed_mb: u64, free_mb: u64 },
}

/// Megabytes that must be free to download and unpack `size_bytes`.
pub fn required_free_mb(size_bytes: u64) -> u64 {
    let size_mb = size_bytes.div_ceil(1024 * 1024);
    size_mb + size_mb * UNPACK_HEADROOM_PERCENT / 100 + MIN_FREE_AFTER_MB
}

pub fn judge_disk_space(free_mb: Option<u64>, size_bytes: Option<u64>) -> DiskVerdict {
    match (free_mb, size_bytes) {
        (Some(free_mb), Some(size)) => {
            let needed_mb = required_free_mb(size);
            if free_mb >= needed_mb {
                DiskVerdict::Enough
            } else {
                DiskVerdict::NotEnough { needed_mb, free_mb }
            }
        }
        _ => DiskVerdict::Unknown,
    }
}

fn gb(mb: u64) -> String {
    format!("{:.1} GB", mb as f64 / 1024.0)
}

/// One short sentence for the status bar. Names the problem and the way out; the folder goes in
/// the log, not the banner.
pub fn not_enough_space_message(model: &str, needed_mb: u64, free_mb: u64) -> String {
    format!(
        "Not enough disk space for {model}: it needs about {} free, only {} available. Free up space or choose a smaller model.",
        gb(needed_mb),
        gb(free_mb)
    )
}

/// `Err(message)` when the drive holding `model_dir` is too full for `size_bytes`.
pub fn check_before_download(model: &str, size_bytes: Option<u64>, model_dir: &Path) -> Result<(), String> {
    check_with_free_space(model, size_bytes, model_dir, crate::preflight::free_disk_mb(model_dir))
}

fn check_with_free_space(model: &str, size_bytes: Option<u64>, model_dir: &Path, free_mb: Option<u64>) -> Result<(), String> {
    match judge_disk_space(free_mb, size_bytes) {
        DiskVerdict::NotEnough { needed_mb, free_mb } => {
            crate::app_log::log(
                "download",
                &format!("refusing to download {model}: needs {needed_mb} MB free, {free_mb} MB free in {}", model_dir.display()),
            );
            Err(not_enough_space_message(model, needed_mb, free_mb))
        }
        DiskVerdict::Unknown => {
            crate::app_log::log("download", &format!("disk space check for {model} skipped (free space or size unknown)"));
            Ok(())
        }
        DiskVerdict::Enough => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn required_space_adds_headroom_and_a_floor() {
        // 4 GiB download: 4096 MB + 20% + 1 GB floor.
        assert_eq!(required_free_mb(4 * GIB), 4096 + 819 + 1024);
    }

    #[test]
    fn verdicts() {
        assert_eq!(judge_disk_space(Some(100_000), Some(4 * GIB)), DiskVerdict::Enough);
        assert_eq!(judge_disk_space(Some(2_000), Some(4 * GIB)), DiskVerdict::NotEnough { needed_mb: 5939, free_mb: 2_000 });
        assert_eq!(judge_disk_space(None, Some(4 * GIB)), DiskVerdict::Unknown);
        assert_eq!(judge_disk_space(Some(10), None), DiskVerdict::Unknown);
    }

    #[test]
    fn message_names_model_sizes_and_a_way_out() {
        let m = not_enough_space_message("qwen3.5-4b", 5939, 2048);
        assert!(m.starts_with("Not enough disk space for qwen3.5-4b"));
        assert!(m.contains("5.8 GB") && m.contains("2.0 GB"));
        assert!(m.contains("smaller model"));
        assert!(m.len() < 200, "must fit the status bar summary without being cut: {}", m.len());
    }

    #[test]
    fn check_refuses_only_when_known_to_be_too_small() {
        let dir = Path::new("/models");
        assert!(check_with_free_space("m", Some(4 * GIB), dir, Some(1000)).is_err());
        assert!(check_with_free_space("m", Some(4 * GIB), dir, Some(50_000)).is_ok());
        assert!(check_with_free_space("m", Some(4 * GIB), dir, None).is_ok());
    }
}
