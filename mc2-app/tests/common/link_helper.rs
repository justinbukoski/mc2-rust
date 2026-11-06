//! Link helpers that fall back to copy when hard_link is unavailable to the test process, including
//! across devices and when the test process is denied permission to create a link.

use std::fs;
use std::io;
use std::path::Path;

/// Returns `true` for cross-device and permission-denied link failures, where copying is a safe
/// fallback for read-only or sandboxed data volumes. Returns `false` for other errors.
///
/// A copy still propagates source-read or destination-write errors; only errors on creating the
/// hard link trigger it.
pub fn should_copy(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::CrossesDevices | io::ErrorKind::PermissionDenied
    )
}

/// Attempts to hard-link `from` to `to`. If the hard-link fails with `ErrorKind::CrossesDevices`
/// (EXDEV) or permission denied, falls back to `std::fs::copy`. Other errors are propagated.
///
/// This allows tests to work when `MC2_DATA` and the temp directory are on different file systems.
/// The fallback preserves file contents but not inode sharing.
pub fn link_or_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    // Try hard link first
    match fs::hard_link(from, to) {
        Ok(()) => Ok(()),
        Err(e) if should_copy(&e) => {
            // Fall back to copy when linking is unavailable or denied.
            fs::copy(from, to).map(|_| ())
        }
        Err(e) => Err(e),
    }
}

/// Recursively mirrors a directory tree from `src` to `dst`. Files are hard-linked (or copied
/// when linking fails across devices or is denied). Directories are created as needed.
///
/// `skip_file` is an optional predicate that returns `true` for files that should not be mirrored.
///
/// This allows tests to work when `MC2_DATA` and the temp directory are on different file systems.
pub fn link_tree(
    src: &Path,
    dst: &Path,
    skip_file: Option<&dyn Fn(&Path) -> bool>,
) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());

        if entry.file_type()?.is_dir() {
            link_tree(&from, &to, skip_file)?;
        } else if let Some(skip) = skip_file {
            if !skip(&from) {
                link_or_copy(&from, &to)?;
            }
        } else {
            link_or_copy(&from, &to)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

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
        link_tree(&src, &dst, None).unwrap();

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
    fn test_link_tree_with_skip() {
        // Create a temporary directory for testing
        let temp_dir = std::env::temp_dir();
        let test_dir = temp_dir.join(format!("link_tree_skip_test_{}", std::process::id()));

        // Clean up if exists
        let _ = fs::remove_dir_all(&test_dir);
        fs::create_dir_all(&test_dir).unwrap();

        // Create source directory
        let src = test_dir.join("src");
        let dst = test_dir.join("dst");

        fs::create_dir_all(&src).unwrap();

        std::fs::File::create(src.join("keep.txt"))
            .unwrap()
            .write_all(b"keep")
            .unwrap();
        std::fs::File::create(src.join("skip.txt"))
            .unwrap()
            .write_all(b"skip")
            .unwrap();

        // Mirror with skip predicate
        link_tree(&src, &dst, Some(&|p| p.file_name().unwrap() == "skip.txt")).unwrap();

        // Verify keep.txt exists but skip.txt doesn't
        assert!(dst.join("keep.txt").exists());
        assert!(!dst.join("skip.txt").exists());

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
    fn test_should_copy_copies_after_permission_denied_but_not_other_errors() {
        // Read-only and sandboxed data volumes can deny hard links while allowing copies.
        let perm_denied_error = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(
            should_copy(&perm_denied_error),
            "permission denial should use the copy fallback for restricted data volumes"
        );

        // Test with NotFound
        let not_found_error = io::Error::from(io::ErrorKind::NotFound);
        assert!(
            !should_copy(&not_found_error),
            "should_copy should return false for ErrorKind::NotFound"
        );
    }
}
