//! Embeds `assets/images/logo.png` as the Windows application icon.
//!
//! GPUI loads the window and taskbar icon from icon resource #1 of the
//! executable, and Explorer shows the same icon for the .exe. Rather than add
//! a resource-compiler crate, this writes the tiny binary `.res` the MSVC
//! linker accepts directly: one PNG-compressed icon image plus the icon group
//! that points at it (PNG icons are supported from Windows Vista).

use std::path::PathBuf;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

/// One resource entry: fixed header, then the data padded to 4 bytes.
fn entry(out: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes()); // header size
    out.extend([0xFF, 0xFF]);
    out.extend(kind.to_le_bytes());
    out.extend([0xFF, 0xFF]);
    out.extend(id.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // data version
    out.extend(0x1030u16.to_le_bytes()); // memory flags
    out.extend(0x0409u16.to_le_bytes()); // language: en-US
    out.extend(0u32.to_le_bytes()); // version
    out.extend(0u32.to_le_bytes()); // characteristics
    out.extend(data);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/images/logo.png");
    let target_msvc = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows")
        && std::env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|env| env == "msvc");
    if !target_msvc {
        return;
    }
    let png = std::fs::read("../../assets/images/logo.png").expect("assets/images/logo.png");
    // The PNG's own size (square, up to 256) goes in the group entry; 0 means 256.
    let width = u32::from_be_bytes([png[16], png[17], png[18], png[19]]);
    let height = u32::from_be_bytes([png[20], png[21], png[22], png[23]]);
    let dim = |v: u32| if v >= 256 { 0u8 } else { v as u8 };

    let mut group = Vec::new();
    group.extend(0u16.to_le_bytes()); // reserved
    group.extend(1u16.to_le_bytes()); // type: icon
    group.extend(1u16.to_le_bytes()); // image count
    group.extend([dim(width), dim(height), 0, 0]); // size, colours, reserved
    group.extend(1u16.to_le_bytes()); // planes
    group.extend(32u16.to_le_bytes()); // bits per pixel
    group.extend((png.len() as u32).to_le_bytes());
    group.extend(1u16.to_le_bytes()); // RT_ICON id

    let mut res = Vec::new();
    // A .res file starts with an empty 32-byte entry.
    res.extend(0u32.to_le_bytes());
    res.extend(32u32.to_le_bytes());
    res.extend([0xFF, 0xFF, 0, 0, 0xFF, 0xFF, 0, 0]);
    res.extend([0u8; 16]);
    entry(&mut res, RT_ICON, 1, &png);
    entry(&mut res, RT_GROUP_ICON, 1, &group);

    let path = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("app-icon.res");
    std::fs::write(&path, res).expect("write app-icon.res");
    println!("cargo:rustc-link-arg-bins={}", path.display());
}
