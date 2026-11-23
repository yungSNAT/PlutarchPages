//! `cargo xtask icons`: regenerate the app icons from `assets/app-icon/pdfcraft.svg` and the
//! Windows PDF file icon from `assets/app-icon/pdfcraft-document.svg`. Both are master vectors.
//!
//! Needs resvg on PATH (`brew install resvg` or `cargo install resvg`); on macOS, iconutil also
//! writes the .icns. The outputs are committed, so building and packaging never need these tools.
//! After running it, update the sha256 values in ATTRIBUTION.toml, then `cargo xtask assets --write`.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{Context, bail};

use crate::gates::{self, PrivateDir};

const ID: &str = "ai.storyteller.pdfcraft";
/// Linux hicolor sizes (full bleed; hicolor/256x256 is also the runtime icon on Windows and Linux).
const HICOLOR: [u32; 8] = [16, 24, 32, 48, 64, 128, 256, 512];
/// Windows .ico entries, PNG-compressed.
const ICO: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];
/// macOS iconset base sizes; each also gets an @2x entry.
const ICNS: [u32; 5] = [16, 32, 128, 256, 512];

pub fn run(args: &[String]) -> anyhow::Result<()> {
    if let Some(arg) = args.first() {
        bail!("unexpected argument `{arg}` (usage: cargo xtask icons)");
    }
    let dir = gates::root().join("assets/app-icon");
    let svg = dir.join("pdfcraft.svg");
    if !on_path("resvg") {
        bail!("resvg not found (brew install resvg)");
    }
    let tmp = PrivateDir::new("pdfcraft-icons")?;

    // The master is a full-bleed 512 tile (rx=112): right for Windows and Linux. macOS icons
    // follow Apple's grid instead: an 824 px body centred on a transparent 1024 canvas.
    let mac = tmp.path().join("macos.svg");
    fs::write(&mac, mac_svg(&svg)).with_context(|| format!("write {}", mac.display()))?;

    // 1024 px PNG on Apple's grid (also the runtime Dock icon on macOS, see apps/pdfcraft/src/main.rs).
    render(&mac, 1024, &dir.join("pdfcraft-1024.png"))?;

    for s in HICOLOR {
        let apps = dir.join(format!("hicolor/{s}x{s}/apps"));
        fs::create_dir_all(&apps).with_context(|| format!("create {}", apps.display()))?;
        render(&svg, s, &apps.join(format!("{ID}.png")))?;
    }
    let scalable = dir.join("hicolor/scalable/apps");
    fs::create_dir_all(&scalable).with_context(|| format!("create {}", scalable.display()))?;
    fs::copy(&svg, scalable.join(format!("{ID}.svg"))).context("copy the scalable icon")?;

    // The document icon has its own resource in pdfcraft.exe; the app icon remains the
    // first/default icon.
    for name in ["pdfcraft", "pdfcraft-document"] {
        let mut pngs = Vec::with_capacity(ICO.len());
        for s in ICO {
            let png = tmp.path().join(format!("{name}-{s}.png"));
            render(&dir.join(format!("{name}.svg")), s, &png)?;
            pngs.push(fs::read(&png).with_context(|| format!("read {}", png.display()))?);
        }
        let ico = dir.join(format!("{name}.ico"));
        fs::write(&ico, ico_bytes(&pngs)?).with_context(|| format!("write {}", ico.display()))?;
    }

    if on_path("iconutil") {
        let set = tmp.path().join("pdfcraft.iconset");
        fs::create_dir_all(&set).with_context(|| format!("create {}", set.display()))?;
        for s in ICNS {
            render(&mac, s, &set.join(format!("icon_{s}x{s}.png")))?;
            render(&mac, s * 2, &set.join(format!("icon_{s}x{s}@2x.png")))?;
        }
        let icns = dir.join("pdfcraft.icns");
        let status = Command::new("iconutil").arg("-c").arg("icns").arg("-o").arg(&icns).arg(&set).status().context("run iconutil")?;
        if !status.success() {
            bail!("iconutil failed ({status})");
        }
    } else {
        eprintln!("warning: iconutil not found (macOS only); pdfcraft.icns not regenerated");
    }
    println!("icons written to {}", dir.display());
    Ok(())
}

/// The 1024 canvas that places the master on Apple's icon grid.
fn mac_svg(master: &Path) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" viewBox=\"0 0 1024 1024\">\n\
         <image x=\"100\" y=\"100\" width=\"824\" height=\"824\" xlink:href=\"{}\"/>\n\
         </svg>\n",
        master.display()
    )
}

fn render(src: &Path, size: u32, out: &Path) -> anyhow::Result<()> {
    let size = size.to_string();
    let status = Command::new("resvg").args(["-w", &size, "-h", &size]).arg(src).arg(out).stdin(Stdio::null()).status().context("run resvg")?;
    if !status.success() {
        bail!("resvg failed on {} at {size} px ({status})", src.display());
    }
    Ok(())
}

/// An .ico holding each PNG as is: a 6-byte header, a 16-byte directory entry per image, then
/// the PNGs in order. Width and height come from each PNG's IHDR; 256 is written as 0.
fn ico_bytes(pngs: &[Vec<u8>]) -> anyhow::Result<Vec<u8>> {
    let count = u16::try_from(pngs.len()).context("too many images for one .ico")?;
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    let mut offset = 6 + 16 * u32::from(count);
    for png in pngs {
        let (Some(w), Some(h)) = (png.get(16..20), png.get(20..24)) else {
            bail!("a rendered PNG is too short to hold its IHDR");
        };
        let w = u32::from_be_bytes(w.try_into()?);
        let h = u32::from_be_bytes(h.try_into()?);
        let len = u32::try_from(png.len()).context("PNG too large for an .ico")?;
        // The low byte, as the format stores 256 as 0.
        out.extend_from_slice(&[w.to_le_bytes()[0], h.to_le_bytes()[0], 0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes()); // colour planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset = offset.checked_add(len).context(".ico larger than 4 GiB")?;
    }
    for png in pngs {
        out.extend_from_slice(png);
    }
    Ok(out)
}

/// Whether `name` is an executable file in a PATH folder. On Windows only `name.exe` counts,
/// because that is the only file `Command::new(name)` runs.
fn on_path(name: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else { return false };
    let file = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    std::env::split_paths(&path).any(|dir| is_executable(&dir.join(&file)))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A PNG signature and IHDR claiming `w` x `h`, padded with `extra` bytes.
    fn png(w: u32, h: u32, extra: u8) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDR".to_vec();
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend(std::iter::repeat_n(extra, usize::from(extra)));
        b
    }

    /// The bytes the retired packaging/icons.sh wrote (its embedded Python's struct packing,
    /// "<HHH" then "<BBBBHHII" per image), for a 16 px PNG followed by a 256 px PNG.
    #[test]
    fn ico_matches_the_retired_script_byte_for_byte() {
        let small = png(16, 16, 3);
        let large = png(256, 256, 5);
        let got = ico_bytes(&[small.clone(), large.clone()]).unwrap();
        let mut want = vec![0, 0, 1, 0, 2, 0];
        want.extend_from_slice(&[16, 16, 0, 0, 1, 0, 32, 0, 27, 0, 0, 0, 38, 0, 0, 0]);
        want.extend_from_slice(&[0, 0, 0, 0, 1, 0, 32, 0, 29, 0, 0, 0, 65, 0, 0, 0]);
        want.extend_from_slice(&small);
        want.extend_from_slice(&large);
        assert_eq!(got, want);
    }

    #[test]
    fn ico_refuses_a_png_without_an_ihdr() {
        let err = ico_bytes(&[b"\x89PNG".to_vec()]).unwrap_err();
        assert!(err.to_string().contains("IHDR"), "{err}");
    }

    #[test]
    fn mac_canvas_matches_the_retired_script() {
        let got = mac_svg(Path::new("/r/assets/app-icon/pdfcraft.svg"));
        assert_eq!(
            got,
            "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" viewBox=\"0 0 1024 1024\">\n\
             <image x=\"100\" y=\"100\" width=\"824\" height=\"824\" xlink:href=\"/r/assets/app-icon/pdfcraft.svg\"/>\n\
             </svg>\n"
        );
    }

    #[test]
    fn a_name_missing_from_path_is_not_found() {
        assert!(!on_path("pdfcraft-no-such-tool-7f3a"));
    }
}
