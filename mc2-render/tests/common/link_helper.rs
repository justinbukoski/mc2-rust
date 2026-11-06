//! Link helpers that fall back to copy when hard_link fails with cross-device errors.

use std::fs;
use std::io;
use std::io::Write;
use std::path::Path;

/// Returns `true` if the error is `ErrorKind::CrossesDevices` (EXDEV), which means the fallback
/// to copy is required. Returns `false` for all other errors.
///
/// This function is used to decide whether to fall back to copy when hard_link fails.
/// Only EXDEV (cross-device link) should trigger the copy fallback; other errors should be propagated.
pub fn should_copy(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::CrossesDevices
}

/// Attempts to hard-link `from` to `to`. If the hard-link fails with `ErrorKind::CrossesDevices`
/// (EXDEV), falls back to `std::fs::copy`. Other errors are propagated.
///
/// This allows tests to work when `MC2_DATA` and the temp directory are on different file systems.
/// The fallback preserves file contents but not inode sharing.
pub fn link_or_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    // Try hard link first
    match fs::hard_link(from, to) {
        Ok(()) => Ok(()),
        Err(e) if should_copy(&e) => {
            // Fall back to copy when files are on different file systems
            fs::copy(from, to).map(|_| ())
        }
        Err(e) => Err(e),
    }
}

/// Recursively mirrors a directory tree from `src` to `dst`. Files are hard-linked (or copied
/// when hard-linking fails with cross-device errors). Directories are created as needed.
///
/// This allows tests to work when `MC2_DATA` and the temp directory are on different file systems.
pub fn link_tree(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());

        if entry.file_type().unwrap().is_dir() {
            link_tree(&from, &to);
        } else if entry.file_name() != "options.cfg" {
            link_or_copy(&from, &to).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_link_or_copy_same_file_system() {
        // Create a temporary directory for testing
        let temp_dir = std::env::temp_dir();
        let test_dir = temp_dir.join(format!("link_helper_test_{}", std::process::id()));

        // Clean up if exists
        let _ = fs::remove_dir_all(&test_dir);
        fs::create_dir_all(&test_dir).unwrap();

        // Create source file
        let src = test_dir.join("source.txt");
        std::fs::File::create(&src)
            .unwrap()
            .write_all(b"test content")
            .unwrap();

        // Try to hard link (should work on same file system)
        let dst = test_dir.join("dest.txt");
        let result = link_or_copy(&src, &dst);

        // On same file system, hard link should succeed
        assert!(
            result.is_ok(),
            "link_or_copy should succeed on same file system"
        );

        // Verify content matches
        assert_eq!(fs::read_to_string(&dst).unwrap(), "test content");

        // Clean up
        let _ = fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_link_tree_creates_tree() {
        // Create a temporary directory for testing
        let temp_dir = std::env::temp_dir();
        let test_dir = temp_dir.join(format!("link_tree_test_{}", std::process::id()));

        // Clean up if exists
        let _ = fs::remove_dir_all(&test_dir);
        fs::create_dir_all(&test_dir).unwrap();

        // Create source directory with nested structure
        let src = test_dir.join("src");
        let dst = test_dir.join("dst");

        fs::create_dir_all(src.join("subdir")).unwrap();

        std::fs::File::create(src.join("file1.txt"))
            .unwrap()
            .write_all(b"file1")
            .unwrap();
        std::fs::File::create(src.join("subdir").join("file2.txt"))
            .unwrap()
            .write_all(b"file2")
            .unwrap();

        // Mirror with link_tree
        link_tree(&src, &dst);

        // Verify structure
        assert!(dst.exists());
        assert!(dst.join("file1.txt").exists());
        assert!(dst.join("subdir").exists());
        assert!(dst.join("subdir").join("file2.txt").exists());

        // Verify content
        assert_eq!(fs::read_to_string(dst.join("file1.txt")).unwrap(), "file1");
        assert_eq!(
            fs::read_to_string(dst.join("subdir").join("file2.txt")).unwrap(),
            "file2"
        );

        // Clean up
        let _ = fs::remove_dir_all(&test_dir);
    }

    #[test]
    fn test_should_copy_returns_true_for_cross_devices() {
        // Test that should_copy returns true for ErrorKind::CrossesDevices (EXDEV)
        // Use platform-neutral io::Error::from(io::ErrorKind::CrossesDevices)
        let cross_device_error = io::Error::from(io::ErrorKind::CrossesDevices);
        assert!(
            should_copy(&cross_device_error),
            "should_copy should return true for ErrorKind::CrossesDevices (EXDEV)"
        );
    }

    #[test]
    fn test_should_copy_returns_false_for_other_errors() {
        // Test that should_copy returns false for other error kinds
        // Use platform-neutral io::Error::from(io::ErrorKind::PermissionDenied)
        let perm_denied_error = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(
            !should_copy(&perm_denied_error),
            "should_copy should return false for PermissionDenied"
        );

        // Test with NotFound
        let not_found_error = io::Error::from(io::ErrorKind::NotFound);
        assert!(
            !should_copy(&not_found_error),
            "should_copy should return false for ErrorKind::NotFound"
        );
    }
}
