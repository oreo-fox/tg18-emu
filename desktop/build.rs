//! Puts the egg icon, the version information (Properties > Details:
//! product name, description, version, copyright) and the application
//! manifest into tg18.exe itself, so
//! Explorer, shortcuts and pinned taskbar buttons show them. Windows reads a program's icon from the
//! resource section (.rsrc) of the .exe. Usually a resource compiler
//! (windres or rc.exe) makes that section; neither comes with the GNU Rust
//! toolchain, so this script writes the small COFF object file by hand and
//! hands it to the linker.

use std::path::PathBuf;

#[allow(dead_code)]
#[path = "src/egg.rs"]
mod egg;

/// Icon sizes in the file: Windows picks the best one for each place.
const SIZES: [usize; 8] = [16, 20, 24, 32, 40, 48, 64, 256];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/egg.rs");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if os != "windows" {
        return;
    }
    // COFF machine type and the "address relative to the image" relocation
    let (machine, reloc) = match arch.as_str() {
        "x86_64" => (0x8664u16, 3u16),
        "x86" => (0x014C, 7),
        "aarch64" => (0xAA64, 2),
        _ => {
            println!("cargo:warning=no exe icon for target arch {}", arch);
            return;
        }
    };
    let images: Vec<Vec<u8>> = SIZES.iter().map(|&s| icon_image(s)).collect();
    let version = version_info();
    let manifest = manifest();
    let obj = coff_resources(machine, reloc, &images, &version, &manifest);
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("tg18-icon.o");
    std::fs::write(&out, obj).expect("write icon object");
    println!("cargo:rustc-link-arg-bins={}", out.display());
}

/// Who made it, as shown in the file's properties.
const AUTHOR: &str = "oreo-fox";

/// The VS_VERSIONINFO block: fixed version numbers plus the text fields
/// Windows shows under Properties > Details. The version comes from
/// desktop/Cargo.toml.
fn version_info() -> Vec<u8> {
    let ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let n: Vec<u32> = ver.split(|c: char| !c.is_ascii_digit()).filter_map(|x| x.parse().ok()).chain([0, 0, 0, 0]).take(4).collect();
    let (ms, ls) = (n[0] << 16 | n[1], n[2] << 16 | n[3]);

    let mut fixed = Vec::new();
    for x in [0xFEEF_04BDu32, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0] {
        put32(&mut fixed, x); // signature, struct version, file + product version, flags, NT, app
    }
    let copyright = format!("Copyright (c) 2026 {}. MIT License.", AUTHOR);
    let strings: Vec<Vec<u8>> = [
        ("CompanyName", AUTHOR),
        ("FileDescription", "tg18-emu - Tamagotchi On/Meets emulator"),
        ("FileVersion", &ver),
        ("InternalName", "tg18"),
        ("LegalCopyright", &copyright),
        ("OriginalFilename", "tg18.exe"),
        ("ProductName", "tg18-emu"),
        ("ProductVersion", &ver),
    ]
    .iter()
    .map(|(k, v)| {
        let text = utf16z(v);
        version_node(k, &text, (text.len() / 2) as u16, 1, &[])
    })
    .collect();
    let table = version_node("040904B0", &[], 0, 1, &strings); // English (US), Unicode
    let string_info = version_node("StringFileInfo", &[], 0, 1, &[table]);
    let translation = version_node("Translation", &0x04B0_0409u32.to_le_bytes(), 4, 0, &[]);
    let var_info = version_node("VarFileInfo", &[], 0, 1, &[translation]);
    version_node("VS_VERSION_INFO", &fixed, fixed.len() as u16, 0, &[string_info, var_info])
}

/// The application manifest nearly every Windows program carries: runs as
/// the user (never asks for admin rights), made for Windows 7 to 11, DPI
/// aware like SetProcessDPIAware, and the current look for message boxes
/// and the folder picker (Common Controls 6).
fn manifest() -> Vec<u8> {
    let ver = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let n: Vec<String> = ver.split(|c: char| !c.is_ascii_digit()).filter(|x| !x.is_empty()).map(String::from).chain(["0".into(), "0".into(), "0".into(), "0".into()]).take(4).collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="{author}.tg18-emu" version="{version}"/>
  <description>tg18-emu - Tamagotchi On/Meets emulator</description>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{35138b9a-5d96-4fbd-8e2d-a2440225f93a}}"/>
      <supportedOS Id="{{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}}"/>
      <supportedOS Id="{{1f676c76-80e1-4239-95bb-83d0f6d0da78}}"/>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
    </windowsSettings>
  </application>
</assembly>
"#,
        author = AUTHOR,
        version = n.join(".")
    )
    .into_bytes()
}

/// One node of the version block: length, value length, type (1 = text),
/// key, value, children, each part starting on a 4-byte boundary.
fn version_node(key: &str, value: &[u8], value_len: u16, kind: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![0, 0];
    put16(&mut v, value_len);
    put16(&mut v, kind);
    v.extend_from_slice(&utf16z(key));
    pad4(&mut v);
    v.extend_from_slice(value);
    for c in children {
        pad4(&mut v);
        v.extend_from_slice(c);
    }
    let len = v.len() as u16;
    v[0..2].copy_from_slice(&len.to_le_bytes());
    v
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect()
}

fn pad4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

/// One icon image: PNG for 256 px (keeps the file small), the classic
/// bitmap format for the small sizes.
fn icon_image(size: usize) -> Vec<u8> {
    let px = egg::egg_pixels(size);
    if size >= 256 {
        return png_rgba(size, &px);
    }
    let mut v = Vec::new();
    // BITMAPINFOHEADER; the height counts the colour and the mask bitmap
    put32(&mut v, 40);
    put32(&mut v, size as u32);
    put32(&mut v, 2 * size as u32);
    put16(&mut v, 1);
    put16(&mut v, 32);
    put32(&mut v, 0); // BI_RGB
    let mask_row = (size + 31) / 32 * 4;
    put32(&mut v, (size * size * 4 + mask_row * size) as u32);
    for _ in 0..4 {
        put32(&mut v, 0);
    }
    // colour rows bottom-up, BGRA (egg_pixels gives 0xAARRGGBB)
    for y in (0..size).rev() {
        for x in 0..size {
            put32(&mut v, px[y * size + x]);
        }
    }
    // AND mask: all 0 (the alpha channel decides transparency)
    v.extend(std::iter::repeat(0).take(mask_row * size));
    v
}

fn png_rgba(size: usize, px: &[u32]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(size * (size * 4 + 1));
    for y in 0..size {
        raw.push(0); // no filter
        for &c in &px[y * size..(y + 1) * size] {
            raw.extend_from_slice(&[(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]);
        }
    }
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&(size as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]); // 8 bit RGBA
    png_chunk(&mut png, b"IHDR", &ihdr);
    png_chunk(&mut png, b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 9));
    png_chunk(&mut png, b"IEND", &[]);
    png
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let start = out.len();
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let crc = crc32(&out[start..]);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

/// A COFF object with one .rsrc section holding the icon images (resource
/// type RT_ICON, ids 1..n), the icon group that lists them (RT_GROUP_ICON,
/// id 1), the version information (RT_VERSION, id 1) and the manifest
/// (RT_MANIFEST, id 1 = read when the program starts). The layout is the standard three-level resource
/// tree: type -> id -> language -> data entry. Data entries hold addresses
/// relative to the final image, so each gets a relocation.
fn coff_resources(machine: u16, reloc_type: u16, images: &[Vec<u8>], version: &[u8], manifest: &[u8]) -> Vec<u8> {
    const RT_ICON: u32 = 3;
    const RT_GROUP_ICON: u32 = 14;
    const RT_VERSION: u32 = 16;
    const RT_MANIFEST: u32 = 24;
    const LANG: u32 = 0x0409; // English (US), like windres's default
    const SUBDIR: u32 = 0x8000_0000;

    // the icon group: header + one 14-byte entry per image
    let mut group = Vec::new();
    put16(&mut group, 0);
    put16(&mut group, 1); // icons
    put16(&mut group, images.len() as u16);
    for (i, (img, &size)) in images.iter().zip(SIZES.iter()).enumerate() {
        let b = if size >= 256 { 0 } else { size as u8 };
        group.extend_from_slice(&[b, b, 0, 0]);
        put16(&mut group, 1); // planes
        put16(&mut group, 32); // bits per pixel
        put32(&mut group, img.len() as u32);
        put16(&mut group, i as u16 + 1);
    }

    // resources as (type, id, data), types in ascending order
    let mut res: Vec<(u32, u32, &[u8])> = images.iter().enumerate().map(|(i, d)| (RT_ICON, i as u32 + 1, &d[..])).collect();
    res.push((RT_GROUP_ICON, 1, &group));
    res.push((RT_VERSION, 1, version));
    res.push((RT_MANIFEST, 1, manifest));
    let types = [RT_ICON, RT_GROUP_ICON, RT_VERSION, RT_MANIFEST];

    // sizes of the tree's parts, to work out the offsets up front
    let dir = |entries: usize| 16 + 8 * entries;
    let root_len = dir(types.len());
    let type_dirs_len: usize = types.iter().map(|t| dir(res.iter().filter(|r| r.0 == *t).count())).sum();
    let lang_dirs_len = res.len() * dir(1);
    let data_entries_at = root_len + type_dirs_len + lang_dirs_len;
    let mut data_at = data_entries_at + res.len() * 16;

    let mut sec = Vec::new();
    let mut relocs: Vec<u32> = Vec::new();
    let dir_header = |sec: &mut Vec<u8>, ids: usize| {
        put32(sec, 0);
        put32(sec, 0);
        put32(sec, 0);
        put16(sec, 0);
        put16(sec, ids as u16);
    };

    // level 1: one entry per type
    dir_header(&mut sec, types.len());
    let mut type_dir_at = root_len;
    for t in types {
        put32(&mut sec, t);
        put32(&mut sec, SUBDIR | type_dir_at as u32);
        type_dir_at += dir(res.iter().filter(|r| r.0 == t).count());
    }
    // level 2: one entry per resource id, pointing at its language directory
    let mut lang_dir_at = root_len + type_dirs_len;
    for t in types {
        let of_type: Vec<_> = res.iter().filter(|r| r.0 == t).collect();
        dir_header(&mut sec, of_type.len());
        for r in of_type {
            put32(&mut sec, r.1);
            put32(&mut sec, SUBDIR | lang_dir_at as u32);
            lang_dir_at += dir(1);
        }
    }
    // level 3: one language each, pointing at the data entry
    for i in 0..res.len() {
        dir_header(&mut sec, 1);
        put32(&mut sec, LANG);
        put32(&mut sec, (data_entries_at + i * 16) as u32);
    }
    // data entries: address (relocated), size, code page, reserved
    assert_eq!(sec.len(), data_entries_at);
    let mut data_offsets = Vec::new();
    for r in &res {
        relocs.push(sec.len() as u32);
        put32(&mut sec, data_at as u32);
        put32(&mut sec, r.2.len() as u32);
        put32(&mut sec, 0);
        put32(&mut sec, 0);
        data_offsets.push(data_at);
        data_at += (r.2.len() + 7) & !7;
    }
    for (r, at) in res.iter().zip(data_offsets) {
        assert_eq!(sec.len(), at);
        sec.extend_from_slice(r.2);
        while sec.len() % 8 != 0 {
            sec.push(0);
        }
    }

    // the object file: header, one section header, data, relocations,
    // a symbol for the section (the relocations refer to it), no strings
    let header_len = 20 + 40;
    let relocs_at = header_len + sec.len();
    let symbols_at = relocs_at + relocs.len() * 10;
    let mut obj = Vec::new();
    put16(&mut obj, machine);
    put16(&mut obj, 1); // sections
    put32(&mut obj, 0); // time stamp
    put32(&mut obj, symbols_at as u32);
    put32(&mut obj, 1); // symbols
    put16(&mut obj, 0); // optional header size
    put16(&mut obj, 0); // characteristics

    obj.extend_from_slice(b".rsrc\0\0\0");
    put32(&mut obj, 0); // virtual size
    put32(&mut obj, 0); // virtual address
    put32(&mut obj, sec.len() as u32);
    put32(&mut obj, header_len as u32);
    put32(&mut obj, relocs_at as u32);
    put32(&mut obj, 0); // line numbers
    put16(&mut obj, relocs.len() as u16);
    put16(&mut obj, 0);
    put32(&mut obj, 0xC000_0040); // initialised data, readable, writable (as windres)

    obj.extend_from_slice(&sec);
    for r in relocs {
        put32(&mut obj, r);
        put32(&mut obj, 0); // symbol 0 = the section
        put16(&mut obj, reloc_type);
    }
    obj.extend_from_slice(b".rsrc\0\0\0");
    put32(&mut obj, 0); // value
    put16(&mut obj, 1); // section number
    put16(&mut obj, 0); // type
    obj.push(3); // static
    obj.push(0); // no aux records
    put32(&mut obj, 4); // empty string table
    obj
}

fn put16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn put32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}
