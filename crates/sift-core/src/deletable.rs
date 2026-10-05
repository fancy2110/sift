//! Whether an entry may be sent to the trash.
//!
//! This is a *policy* question, not a permission probe: a file can be writable
//! and still be something the product refuses to touch, and a volume can be
//! read-only without any single path looking wrong. The rules stay here so both
//! front ends disable exactly the same rows.

use std::path::{Component, Path};

/// The volume root and every one of its ancestors are never deletion targets.
///
/// On macOS the synthetic mount directory `/Volumes` exists only to hold mount
/// points, so a *direct child* of it is by definition a mount point and is
/// refused the same way. Its deeper descendants are ordinary files.
pub fn is_volume_root(path: &Path) -> bool {
    // "/" on Unix, "C:\" on Windows, or any mount point with no file name.
    if path.file_name().is_none() {
        return true;
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(parent) = path.parent() {
            return parent == Path::new("/Volumes");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
    }
    false
}

/// Whether `path` sits inside a system directory the product refuses to touch.
pub fn is_protected_location(path: &Path) -> bool {
    #[cfg(unix)]
    {
        // Allocation-free match on the first path component; the refused roots
        // are all top-level except `/Library/Apple`, which needs one more.
        let mut components = path.components();
        if !matches!(components.next(), Some(Component::RootDir)) {
            return false;
        }
        let Some(Component::Normal(first)) = components.next() else {
            return false;
        };
        match first.as_encoded_bytes() {
            b"System" | b"bin" | b"sbin" | b"usr" | b"private" | b"dev" | b"etc" => true,
            b"Library" => matches!(
                components.next(),
                Some(Component::Normal(second)) if second.as_encoded_bytes() == b"Apple"
            ),
            _ => false,
        }
    }
    #[cfg(windows)]
    {
        let mut components = path.components();
        let Some(Component::Prefix(_)) = components.next() else {
            return false;
        };
        if !matches!(components.next(), Some(Component::RootDir)) {
            return false;
        }
        let Some(Component::Normal(first)) = components.next() else {
            return false;
        };
        match first.to_string_lossy().to_lowercase().as_str() {
            "windows" | "programdata" | "recovery" | "program files"
            | "program files (x86)" | "$recycle.bin" => true,
            _ => false,
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        true
    }
}

/// Whether `path` contains a `..` component or is otherwise not a clean
/// absolute path. Such a path must never be offered for deletion.
pub fn is_traversal(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::CurDir | Component::Prefix(_)
        )
    })
}

/// Bundle and OS-container suffixes.
const BUNDLE_SUFFIXES: [&str; 10] = [
    ".app",
    ".framework",
    ".kext",
    ".bundle",
    ".xpc",
    ".appex",
    ".plugin",
    ".prefpane",
    ".mdimporter",
    ".qlgenerator",
];

/// Whether the entry itself is an application bundle or OS-owned container that a
/// space tool should present but not delete on the user's behalf.
pub fn is_bundle(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name_is_bundle(name.as_bytes())
}

/// Whether a raw file name ends with an application/OS-container suffix.
#[inline]
fn name_is_bundle(name: &[u8]) -> bool {
    BUNDLE_SUFFIXES.iter().any(|suffix| name.ends_with(suffix.as_bytes()))
}

/// Whether *any* component of `path` is a bundle.
///
/// The distinction matters for classification: a `node_modules` inside a
/// developer's project is a regenerable cache, but the one inside
/// `ChatGPT.app/Contents/Resources/…` is part of an installed application that
/// the user cannot rebuild. Same name, same directory shape, opposite conclusion
/// — so the check has to look at the whole path, not just the last component.
pub fn inside_bundle(path: &Path) -> bool {
    path.components().any(|component| {
        let text = component.as_os_str().to_string_lossy();
        BUNDLE_SUFFIXES.iter().any(|suffix| text.ends_with(suffix))
    })
}

/// Final verdict for one entry, with the reason shown to the user when the
/// answer is no. The reason is an i18n key, never a formatted sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Deletable {
    Yes,
    /// Refused because of where it lives.
    ProtectedLocation,
    /// Refused because it is the volume root.
    VolumeRoot,
    /// Refused because the path is not a clean absolute path.
    UnresolvedPath,
    /// Refused because the process cannot write to the parent directory.
    ParentNotWritable,
    /// Refused because the entry is a bundle the product will not dissolve.
    Bundle,
    /// Refused because the entry carries an immutable flag (`uchg`/`schg`).
    ImmutableFlag,
}

impl Deletable {
    pub const fn is_yes(self) -> bool {
        matches!(self, Deletable::Yes)
    }

    /// i18n key explaining a refusal, or `None` when deletion is allowed.
    pub const fn refusal_key(self) -> Option<&'static str> {
        match self {
            Deletable::Yes => None,
            Deletable::ProtectedLocation => Some("delete.refused.protectedLocation"),
            Deletable::VolumeRoot => Some("delete.refused.volumeRoot"),
            Deletable::UnresolvedPath => Some("delete.refused.unresolvedPath"),
            Deletable::ParentNotWritable => Some("delete.refused.parentNotWritable"),
            Deletable::Bundle => Some("delete.refused.bundle"),
            Deletable::ImmutableFlag => Some("delete.refused.immutableFlag"),
        }
    }
}

/// Policy-only verdict, without touching the filesystem.
pub fn classify(path: &Path) -> Deletable {
    if is_volume_root(path) {
        return Deletable::VolumeRoot;
    }
    if is_traversal(path) && !path.is_absolute() {
        return Deletable::UnresolvedPath;
    }
    // The mount directory itself is a system object; its children are handled
    // by `is_volume_root` above.
    #[cfg(target_os = "macos")]
    if path == Path::new("/Volumes") {
        return Deletable::ProtectedLocation;
    }
    if is_protected_location(path) {
        return Deletable::ProtectedLocation;
    }
    if is_bundle(path) {
        return Deletable::Bundle;
    }
    Deletable::Yes
}

/// Policy verdict for one child of `parent_path`, without building the joined
/// path or allocating. `parent_protected` is [`is_protected_location`] on the
/// parent itself; when true, every descendant inherits the refusal.
#[inline]
pub fn classify_child(
    parent_path: &str,
    name: &[u8],
    parent_protected: bool,
) -> Deletable {
    // A direct child of /Volumes is a mount point, not a removable entry.
    if parent_path == "/Volumes" {
        return Deletable::VolumeRoot;
    }
    if parent_protected {
        return Deletable::ProtectedLocation;
    }
    // The only protected root nested one level under a non-protected parent.
    if parent_path == "/Library" && name == b"Apple" {
        return Deletable::ProtectedLocation;
    }
    // At the filesystem root the entry name is the first path component.
    if parent_path == "/" {
        match name {
            b"System" | b"bin" | b"sbin" | b"usr" | b"private" | b"dev" | b"etc" => {
                return Deletable::ProtectedLocation;
            }
            _ => {}
        }
    }
    if name_is_bundle(name) {
        return Deletable::Bundle;
    }
    Deletable::Yes
}

/// Policy verdict plus the real permission probe on the parent directory.
///
/// Checking the *parent* is deliberate: removing an entry changes the parent's
/// contents, so a read-only file in a writable directory is deletable while a
/// writable file in a read-only directory is not.
pub fn classify_with_permissions(path: &Path) -> Deletable {
    let policy = classify(path);
    if !policy.is_yes() {
        return policy;
    }
    // `unlink`/`rename` on an immutable entry fails with EPERM no matter what
    // the parent directory allows, so this is checked first and carries the
    // more actionable reason (clear the flag, then retry).
    #[cfg(target_os = "macos")]
    if entry_is_immutable(path) {
        return Deletable::ImmutableFlag;
    }
    match parent_is_writable(path) {
        true => Deletable::Yes,
        false => Deletable::ParentNotWritable,
    }
}

/// Whether the entry itself carries a user- or system-level immutable flag.
///
/// On macOS `UF_IMMUTABLE` (`uchg`) or `SF_IMMUTABLE` (`schg`) blocks exactly
/// the operations a move-to-trash needs: the entry can be neither renamed nor
/// unlinked while the flag is set, even from a writable parent directory.
/// Symlinks are probed without following them — the flag that matters lives on
/// the link's own inode, not on its target.
#[cfg(target_os = "macos")]
fn entry_is_immutable(path: &Path) -> bool {
    // `st_flags` lives in the Darwin-specific extension on this toolchain.
    use std::os::darwin::fs::MetadataExt;

    /// User-set immutable flag (`chflags uchg`).
    const UF_IMMUTABLE: u32 = 0x0000_0002;
    /// System-set immutable flag (`chflags schg`).
    const SF_IMMUTABLE: u32 = 0x0002_0000;

    match std::fs::symlink_metadata(path) {
        Ok(meta) => meta.st_flags() & (UF_IMMUTABLE | SF_IMMUTABLE) != 0,
        // Unreadable entries stay on the existing path: the parent probe or the
        // per-item deletion error reports them; never silently allow one here.
        Err(_) => false,
    }
}

#[cfg(unix)]
pub fn parent_is_writable(path: &Path) -> bool {
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    let Some(parent) = path.parent() else {
        return false;
    };
    let Ok(meta) = fs::metadata(parent) else {
        return false;
    };
    unix_mode_is_writable_for(meta.mode(), meta.uid(), meta.gid())
}

/// Whether the given Unix mode grants *this process* write permission, given
/// the entry's owning uid/gid.
///
/// The check follows the kernel's owner → group → other order. Crucially the
/// group step tests membership against **every supplementary group**
/// (`getgroups`), not only the effective gid: e.g. `/Applications` is owned
/// by `root:admin` with group-write on, and a normal user belongs to `admin`
/// via a supplementary group, so it must read as writable even though the
/// process's primary group is something else.
///
/// Shared by every write probe in the workspace (core and platform) so the
/// answer stays identical whether bits came from metadata, a bulk-read entry,
/// or an fstat-ed descriptor.
#[cfg(unix)]
pub fn unix_mode_is_writable_for(mode: u32, uid: u32, gid: u32) -> bool {
    if uid == identity::effective_uid() {
        mode & 0o200 != 0
    } else if identity::in_effective_group(gid) {
        mode & 0o020 != 0
    } else {
        mode & 0o002 != 0
    }
}

#[cfg(unix)]
mod identity {
    use std::sync::OnceLock;

    extern "C" {
        fn geteuid() -> u32;
        fn getgroups(size: i32, list: *mut u32) -> i32;
    }

    pub fn effective_uid() -> u32 {
        unsafe { geteuid() }
    }

    /// Whether `gid` is among the process's effective group set.
    pub fn in_effective_group(gid: u32) -> bool {
        effective_groups().contains(&gid)
    }

    /// Test-only view of the cached group set.
    #[cfg(test)]
    pub fn test_groups() -> &'static [u32] {
        effective_groups()
    }

    /// The process's own groups never change; fetch them once.
    fn effective_groups() -> &'static [u32] {
        GROUPS.get_or_init(|| {
            unsafe {
                // size 0 returns the group count without writing the list.
                let count = getgroups(0, std::ptr::null_mut());
                if count <= 0 {
                    return Vec::new();
                }
                let mut list = vec![0u32; count as usize];
                let filled = getgroups(count, list.as_mut_ptr());
                if filled <= 0 {
                    return Vec::new();
                }
                list.truncate(filled as usize);
                list
            }
        })
    }

    static GROUPS: OnceLock<Vec<u32>> = OnceLock::new();
}

/// Whether a *directory itself* permits deleting its children, judged from the
/// directory's own already-fetched metadata (no extra syscall).
///
/// Removing a child entry needs the owner/group/other write bit on the
/// directory that holds it. Callers that have just stat-ed the directory can
/// reuse the metadata here instead of probing again.
#[cfg(unix)]
pub fn dir_is_writable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    unix_mode_is_writable_for(meta.mode(), meta.uid(), meta.gid())
}

/// Windows ACLs are not expressible through Unix mode bits; reaching the
/// metadata proves the directory is accessible, and a move-to-trash reports a
/// per-item error if the ACL ultimately refuses.
#[cfg(windows)]
pub fn dir_is_writable(meta: &std::fs::Metadata) -> bool {
    let _ = meta;
    true
}

#[cfg(windows)]
pub fn parent_is_writable(path: &Path) -> bool {
    use std::fs;
    let Some(parent) = path.parent() else {
        return false;
    };
    // Windows ACLs are not expressible through the Unix mode bits; a metadata
    // call proves the directory is reachable, and the actual move-to-trash
    // reports a per-item error if the ACL refuses.
    fs::metadata(parent).is_ok()
}

#[cfg(not(any(unix, windows)))]
pub fn parent_is_writable(_path: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn volume_root_is_refused() {
        assert_eq!(classify(Path::new("/")), Deletable::VolumeRoot);
        // A mount point under /Volumes is a volume root; its contents are not.
        #[cfg(target_os = "macos")]
        {
            assert_eq!(
                classify(Path::new("/Volumes/Backup")),
                Deletable::VolumeRoot
            );
            assert_eq!(
                classify(Path::new("/Volumes")),
                Deletable::ProtectedLocation
            );
            assert!(classify(Path::new("/Volumes/Backup/Users/me")).is_yes());
        }
    }

    #[test]
    fn system_locations_are_refused() {
        for path in [
            "/System/Library/CoreServices",
            "/usr/bin",
            "/bin",
            "/etc",
            "/private/var",
        ] {
            assert_eq!(
                classify(Path::new(path)),
                Deletable::ProtectedLocation,
                "{path} should be protected"
            );
        }
    }

    #[test]
    fn a_name_that_merely_starts_with_a_protected_prefix_is_allowed() {
        // "/bin" is protected; "/binary-notes" is not, and a naive
        // `starts_with` on the string would wrongly refuse it.
        assert!(classify(Path::new("/Users/me/binary-notes")).is_yes());
        assert!(classify(Path::new("/Users/me/system-design.pdf")).is_yes());
        assert!(classify(Path::new("/Users/me/etcetera")).is_yes());
    }

    #[test]
    fn user_data_is_deletable() {
        for path in [
            "/Users/me/Downloads/dmg",
            "/Users/me/Library/Caches",
            "/Applications/Xcode.app/Contents/Developer",
        ] {
            assert!(classify(Path::new(path)).is_yes(), "{path}");
        }
    }

    #[test]
    fn bundles_are_presented_but_not_dissolved() {
        assert_eq!(
            classify(Path::new("/Applications/Notes.app")),
            Deletable::Bundle
        );
        assert_eq!(
            classify(Path::new("/Users/me/Lib.framework")),
            Deletable::Bundle
        );
        assert!(classify(Path::new("/Users/me/notes.txt")).is_yes());
    }

    #[test]
    fn bundle_membership_is_checked_through_the_whole_path() {
        assert!(inside_bundle(Path::new("/Applications/ChatGPT.app")));
        assert!(inside_bundle(Path::new(
            "/Applications/ChatGPT.app/Contents/Resources/node_modules"
        )));
        assert!(inside_bundle(Path::new(
            "/System/Library/Frameworks/AppKit.framework/Versions/A"
        )));
        // A directory merely named like one is not inside a bundle.
        assert!(!inside_bundle(Path::new("/Users/me/project/node_modules")));
        assert!(!inside_bundle(Path::new("/Users/me/apples")));
        // Only the final component decides `is_bundle`.
        assert!(!is_bundle(Path::new(
            "/Applications/ChatGPT.app/Contents/Resources/node_modules"
        )));
    }

    #[test]
    fn traversal_paths_are_refused() {
        assert!(is_traversal(Path::new("../../etc/passwd")));
        assert!(is_traversal(Path::new("./relative")));
        assert!(!is_traversal(Path::new("/Users/me/file")));
        assert_eq!(classify(Path::new("../escape")), Deletable::UnresolvedPath);
    }

    #[test]
    fn refusals_carry_a_translation_key() {
        assert_eq!(Deletable::Yes.refusal_key(), None);
        for verdict in [
            Deletable::ProtectedLocation,
            Deletable::VolumeRoot,
            Deletable::UnresolvedPath,
            Deletable::ParentNotWritable,
            Deletable::Bundle,
            Deletable::ImmutableFlag,
        ] {
            let key = verdict.refusal_key().expect("refusal needs a key");
            assert!(key.starts_with("delete.refused."), "{key}");
        }
    }

    /// A real directory in the temp dir: policy allows the child, and the
    /// parent's mode decides.
    #[test]
    fn permission_probe_reads_the_parent() {
        use std::fs;
        let dir: PathBuf = std::env::temp_dir().join(format!("sift-del-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let child = dir.join("junk.txt");
        fs::write(&child, b"x").unwrap();

        assert_eq!(classify_with_permissions(&child), Deletable::Yes);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
            assert_eq!(
                classify_with_permissions(&child),
                Deletable::ParentNotWritable
            );
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        }

        let _ = fs::remove_dir_all(&dir);
    }

    /// `uchg` on the entry itself blocks rename/unlink even though the parent
    /// directory is writable; the classifier must say so for both files and
    /// directories.
    #[cfg(target_os = "macos")]
    #[test]
    fn immutable_entries_are_refused_despite_a_writable_parent() {
        use std::fs;
        let dir: PathBuf =
            std::env::temp_dir().join(format!("sift-immutable-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        let file = dir.join("locked.txt");
        fs::write(&file, b"x").unwrap();
        let subdir = dir.join("locked-dir");
        fs::create_dir_all(&subdir).unwrap();
        fs::write(subdir.join("inside.txt"), b"y").unwrap();

        for target in [&file, &subdir] {
            assert_eq!(classify_with_permissions(target), Deletable::Yes);
            chflags(target, "uchg");
            // The kernel refuses the rename a trash move would need.
            let renamed = target.with_extension("moved");
            assert!(fs::rename(target, &renamed).is_err());
            assert_eq!(
                classify_with_permissions(target),
                Deletable::ImmutableFlag,
                "{} should be immutable",
                target.display()
            );
            chflags(target, "nouchg");
            assert_eq!(classify_with_permissions(target), Deletable::Yes);
        }

        let _ = fs::remove_dir_all(&dir);
    }

    /// Set or clear a file flag through the system `chflags` tool.
    #[cfg(target_os = "macos")]
    fn chflags(path: &std::path::Path, flag: &str) {
        let output = std::process::Command::new("chflags")
            .arg(flag)
            .arg(path)
            .output()
            .expect("run chflags");
        assert!(
            output.status.success(),
            "chflags {flag} {} failed: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    #[cfg(unix)]
    #[test]
    fn owner_bit_decides_when_we_own_the_entry() {
        let euid = identity::effective_uid();
        assert!(unix_mode_is_writable_for(0o700, euid, 0));
        assert!(unix_mode_is_writable_for(0o200, euid, 0));
        // Without the owner write bit the group/other bits are ignored,
        // exactly as the kernel does.
        assert!(!unix_mode_is_writable_for(0o077, euid, 0));
        assert!(!unix_mode_is_writable_for(0o022, euid, 0));
    }

    #[cfg(unix)]
    #[test]
    fn group_write_uses_every_supplementary_group() {
        let euid = identity::effective_uid();
        let other_uid = euid.wrapping_add(1);
        let groups = identity::test_groups();
        // getgroups always includes at least the effective group.
        let member = *groups.first().expect("getgroups returned no groups");

        // The /Applications case: owned by someone else (root), group is a
        // group we belong to via a supplementary entry, mode 0775.
        assert!(unix_mode_is_writable_for(0o775, other_uid, member));
        // Membership alone is not enough — the group write bit must be set.
        assert!(!unix_mode_is_writable_for(0o755, other_uid, member));
        assert!(!unix_mode_is_writable_for(0o705, other_uid, member));

        // A gid outside our group set never unlocks the group bit...
        let outsider = (0u32..).find(|g| !groups.contains(g)).unwrap();
        assert!(!unix_mode_is_writable_for(0o775, other_uid, outsider));
        // ...but the owner→group→other fallthrough still reaches `other`.
        assert!(unix_mode_is_writable_for(0o777, other_uid, outsider));
        assert!(unix_mode_is_writable_for(0o002, other_uid, outsider));
        assert!(!unix_mode_is_writable_for(0o005, other_uid, outsider));
    }
}
