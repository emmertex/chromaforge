//! [`crate::folders::volume_key`]: which volume a file is read from, for the engine's I/O
//! scheduler, from the path and a mount list alone.

use crate::folders::{MAX_MOUNTS, Mounts, volume_key};

fn id(path: &str, m: &Mounts) -> String {
    volume_key(path, m).id().to_string()
}

fn case_sensitive_none() -> Mounts {
    Mounts::from_points(Vec::<String>::new(), false)
}

/// With no list, `place()`'s rules decide, for every prefix form it knows.
#[test]
fn without_a_list_place_decides_every_prefix_form() {
    let m = case_sensitive_none();
    let cases = [
        ("/Volumes/archive2/photos/a.nef", "/Volumes/archive2"),
        ("/run/media/me/usb/a.jpg", "/run/media/me/usb"),
        ("/media/me/card/DCIM/a.jpg", "/media/me/card"),
        ("/mnt/disk/a.jpg", "/mnt/disk"),
        ("/run/user/1000/gvfs/smb-share:server=tokyo,share=photos/a.jpg", "/run/user/1000/gvfs/smb-share:server=tokyo,share=photos"),
        ("/net/host/a.jpg", "/net/host"),
        ("/Network/Servers/host/a.jpg", "/Network/Servers/host"),
        ("//server/share/a.jpg", "//server/share"),
        ("D:/photos/a.jpg", "d:"),
        ("/Users/me/Pictures/a.jpg", "/"),
        ("/", "/"),
    ];
    for (path, want) in cases {
        assert_eq!(id(path, &m), want, "{path}");
    }
}

/// Windows verbatim forms reduce to the drive or share; drive letters and shares fold case.
#[test]
fn verbatim_forms_and_drive_letters() {
    let m = case_sensitive_none();
    assert_eq!(id(r"\\?\D:\photos\a.jpg", &m), "d:");
    assert_eq!(id(r"\\?\UNC\Server\Share\a.jpg", &m), "//server/share");
    assert_eq!(id(r"\\Server\Share\x\a.jpg", &m), "//server/share");
    assert_eq!(volume_key(r"\\?\UNC\Server\Share\a.jpg", &m), volume_key("//server/share/b.jpg", &m));
    assert_eq!(volume_key("d:/a", &m), volume_key("D:\\b", &m));
    assert_eq!(volume_key(r"\\?\UNC\Server\Share\a.jpg", &m).name(), "//Server/Share");
    // a server alone keys as itself
    assert_eq!(id("//server/", &m), "//server");
}

/// The longest listed mount point holding the path wins, compared by whole names.
#[test]
fn longest_mount_by_whole_components() {
    let m = Mounts::from_points(["/", "/srv", "/srv/nas", "/Volumes/Photo"], false);
    assert_eq!(id("/srv/nas/2024/a.nef", &m), "/srv/nas");
    assert_eq!(id("/srv/nasty/a.nef", &m), "/srv");
    assert_eq!(id("/srv/nas", &m), "/srv/nas");
    assert_eq!(id("/home/me/a.jpg", &m), "/");
    assert_eq!(id("/Volumes/Photo/a.jpg", &m), "/Volumes/Photo");
    // `/Volumes/Photo` never holds `/Volumes/Photos 2`: the listed `/` does
    assert_eq!(id("/Volumes/Photos 2/a.jpg", &m), "/");
    // without `/` listed, an unmatched path falls back to place()
    let m = Mounts::from_points(["/Volumes/Photo"], false);
    assert_eq!(id("/Volumes/Photos 2/a.jpg", &m), "/Volumes/Photos 2");
}

/// Case folding: macOS lists compare without case and keep the first spelling for messages.
#[test]
fn case_folding_follows_the_list() {
    let mac = Mounts::from_volume_names(["Archive2"]);
    let a = volume_key("/Volumes/archive2/a.nef", &mac);
    let b = volume_key("/Volumes/ARCHIVE2/b.nef", &mac);
    assert_eq!(a, b);
    assert_eq!(a.name(), "/Volumes/archive2");
    // a macOS path that matches nothing listed: place(), folded too
    assert_eq!(volume_key("/Volumes/Other/a", &mac), volume_key("/Volumes/OTHER/b", &mac));
    // Linux compares case: two keys
    let linux = Mounts::from_points(["/", "/srv/Nas", "/srv/nas"], false);
    assert_ne!(volume_key("/srv/Nas/a", &linux), volume_key("/srv/nas/a", &linux));
}

/// On macOS the names in `/Volumes` give the same keys as `place()` (the helper only costs a
/// little there).
#[test]
fn macos_list_matches_place() {
    let mac = Mounts::from_volume_names(["archive2", "archive3", "Macintosh HD"]);
    let none = Mounts::from_points(Vec::<String>::new(), true);
    for p in ["/Volumes/archive2/x/a.nef", "/Volumes/archive3/a", "/Users/me/a.jpg", "/Volumes/Macintosh HD/Users/a"] {
        assert_eq!(volume_key(p, &mac), volume_key(p, &none), "{p}");
    }
    // names that are no volume are left out
    let odd = Mounts::from_volume_names(["", ".", "..", "a/b"]);
    assert!(odd.is_empty());
}

/// mountinfo samples (proc(5)), octal escapes decoded.
#[test]
fn linux_mountinfo_with_escapes() {
    let text = "\
22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
36 22 98:0 /mnt1 /mnt/parent rw,noatime master:1 - ext3 /dev/root rw,errors=continue
40 22 0:44 / /srv/my\\040nas rw,relatime shared:30 - cifs //nas/photos rw
41 22 0:45 / /media/me/CARD\\134X rw - vfat /dev/sdb1 rw
42 22 0:46 / /tab\\011dir rw - tmpfs tmpfs rw
";
    let m = Mounts::from_mountinfo(text);
    assert!(m.is_read());
    assert_eq!(m.len(), 5);
    assert_eq!(m.skipped(), 0);
    assert_eq!(id("/srv/my nas/2024/a.nef", &m), "/srv/my nas");
    // a backslash is a separator here, as for every folder identity (`folder_key`)
    assert_eq!(id("/media/me/CARD\\X/a.jpg", &m), "/media/me/CARD/X");
    assert_eq!(id("/mnt/parent/a", &m), "/mnt/parent");
    assert_eq!(id("/tab\tdir/a", &m), "/tab\tdir");
    // `/srv/nas` mounted somewhere place() doesn't know: keyed by the list, not as `/`
    let m = Mounts::from_mountinfo("1 0 0:1 / / rw - ext4 a rw\n2 1 0:2 / /srv/nas rw - nfs b rw\n");
    assert_eq!(id("/srv/nas/x.jpg", &m), "/srv/nas");
    assert_eq!(id("/srv/nas/x.jpg", &case_sensitive_none()), "/");
}

/// Hostile listings: malformed lines are counted and skipped, bad escapes stay literal, huge
/// listings are cut and say so, nothing panics.
#[test]
fn hostile_mountinfo_never_panics() {
    let m = Mounts::from_mountinfo("garbage\n1 2 3\n\n   \n1 0 0:1 / relative rw - x y z\n");
    assert_eq!(m.len(), 0);
    assert_eq!(m.skipped(), 3, "two short lines and one relative mount point");
    for field in ["/a\\", "/a\\0", "/a\\04", "/a\\777", "/a\\08x", "/\\\\\\", "/\u{fffd}\\377"] {
        let line = format!("1 0 0:1 / {field} rw - x y z\n");
        let _ = Mounts::from_mountinfo(&line);
    }
    let m = Mounts::from_mountinfo("1 0 0:1 / /a\\777b rw - x y z\n");
    assert_eq!(id("/a\\777b/c", &m), "/a/777b", "out-of-range escape kept literally (and read as a separator)");
    let m = Mounts::from_mountinfo("1 0 0:1 / /x\\377 rw - x y z\n");
    assert_eq!(m.len(), 1, "a non-UTF-8 byte is replaced, not refused");
    let mut big = String::new();
    for i in 0..MAX_MOUNTS + 3 {
        big.push_str(&format!("{i} 0 0:1 / /m{i} rw - x y z\n"));
    }
    let m = Mounts::from_mountinfo(&big);
    assert_eq!(m.len(), MAX_MOUNTS);
    assert!(m.truncated());
}

/// Paths that name no volume, and other odd inputs, all get a key without panicking.
#[test]
fn odd_paths_get_a_key() {
    let m = Mounts::from_points(["/"], false);
    let none = volume_key("", &m);
    assert_eq!(none, volume_key("relative/a.jpg", &m));
    assert_eq!(none.id(), "");
    for p in ["//", "///", "\\\\", "\\\\?\\", "\\\\?\\UNC\\", "C:", "1:", "/\0/a", "/../..", &"/a".repeat(10_000)] {
        let _ = volume_key(p, &m);
        let _ = volume_key(p, &case_sensitive_none());
    }
    assert_eq!(id("/../../srv/a", &Mounts::from_points(["/", "/srv"], false)), "/srv");
}

/// `Mounts::none()` reads nothing and decides by `place()`.
#[test]
fn none_is_unread() {
    let m = Mounts::none();
    assert!(!m.is_read());
    assert_eq!(volume_key("/Volumes/x/a", &m).name(), "/Volumes/x");
}
