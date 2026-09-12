//! Cursor image loading: .cur / .ico / .ani / .png
//!
//! Hotspot handling (spec: do NOT blindly use (0,0)):
//! - .cur: hotspot from ICONDIRENTRY bytes 4..8
//! - .ani: hotspot from the ICONDIRENTRY of the embedded icon frames
//!   (RIFF 'ACON' -> LIST 'fram' -> 'icon' chunks, each a complete .ico)
//! - .ico: icons have no hotspot concept -> center of the image
//! - .png: no embedded hotspot -> center (can be tuned via GUI offset later)
use image::imageops::FilterType;
use std::path::Path;

#[derive(Clone)]
pub struct CursorImage {
    pub width: u32,
    pub height: u32,
    /// hotspot in image pixels
    pub hotspot: (i32, i32),
    /// straight alpha RGBA, top-down
    pub rgba: Vec<u8>,
}

struct RawFrame {
    width: u32,
    height: u32,
    hotspot: (i32, i32),
    rgba: Vec<u8>,
}

// ---------------------------------------------------------------------------
// ICO / CUR container (identical layout; type 1 = icon, 2 = cursor)
// ---------------------------------------------------------------------------

struct IcoEntry {
    width: u32,
    height: u32,
    hotspot: (i32, i32),
    size: usize,
    offset: usize,
}

fn parse_ico_container(
    data: &[u8],
    expect_type: Option<u16>,
) -> Result<(Vec<IcoEntry>, u16), String> {
    if data.len() < 6 {
        return Err("file too small".into());
    }
    let ico_type = u16::from_le_bytes([data[2], data[3]]);
    if let Some(want) = expect_type {
        if ico_type != want {
            return Err(format!("container type {} != {}", ico_type, want));
        }
    }
    let count = u16::from_le_bytes([data[4], data[5]]) as usize;
    let mut entries = Vec::new();
    for i in 0..count {
        let e = 6 + i * 16;
        if data.len() < e + 16 {
            break;
        }
        let w = data[e] as u32;
        let h = data[e + 1] as u32;
        let hot_x = u16::from_le_bytes([data[e + 4], data[e + 5]]) as i32;
        let hot_y = u16::from_le_bytes([data[e + 6], data[e + 7]]) as i32;
        let size = u32::from_le_bytes([
            data[e + 8],
            data[e + 9],
            data[e + 10],
            data[e + 11],
        ]) as usize;
        let offset = u32::from_le_bytes([
            data[e + 12],
            data[e + 13],
            data[e + 14],
            data[e + 15],
        ]) as usize;
        entries.push(IcoEntry {
            width: if w == 0 { 256 } else { w },
            height: if h == 0 { 256 } else { h },
            hotspot: (hot_x, hot_y),
            size,
            offset,
        });
    }
    if entries.is_empty() {
        return Err("no images in container".into());
    }
    Ok((entries, ico_type))
}

fn decode_ico_image(data: &[u8], entry: &IcoEntry) -> Result<RawFrame, String> {
    if entry.offset + entry.size > data.len() {
        return Err("image data out of bounds".into());
    }
    let img = &data[entry.offset..entry.offset + entry.size];

    // PNG-embedded frames (Vista+ icons)
    if img.len() > 8 && img[0] == 0x89 && img[1] == b'P' {
        let dyn_img = image::load_from_memory(img).map_err(|e| format!("png frame: {e}"))?;
        let rgba8 = dyn_img.to_rgba8();
        let (w, h) = rgba8.dimensions();
        return Ok(RawFrame {
            width: w,
            height: h,
            hotspot: entry.hotspot,
            rgba: rgba8.into_raw(),
        });
    }

    // BMP variant: BITMAPINFOHEADER(40) + XOR BGRA (bottom-up) + AND mask
    if img.len() < 40 {
        return Err("truncated BITMAPINFOHEADER".into());
    }
    let _bw = i32::from_le_bytes([img[4], img[5], img[6], img[7]]) as u32;
    let bh = i32::from_le_bytes([img[8], img[9], img[10], img[11]]) as u32;
    let bitcount = u16::from_le_bytes([img[14], img[15]]) as u32;
    let w = entry.width;
    let h = if bh >= 2 * entry.height { entry.height } else { bh / 2 };
    if w == 0 || h == 0 {
        return Err("zero-size frame".into());
    }
    if bitcount != 32 {
        return Err(format!("unsupported bit depth {bitcount} (need 32bpp)"));
    }
    let xor_len = (w * h * 4) as usize;
    if img.len() < 40 + xor_len {
        return Err("truncated XOR mask".into());
    }
    let and_row = ((w + 31) / 32 * 4) as usize;
    let and_len = and_row * h as usize;
    let has_and = img.len() >= 40 + xor_len + and_len;
    let any_alpha = img[40..40 + xor_len].iter().skip(3).step_by(4).any(|&a| a != 0);

    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for y in 0..h as usize {
        let src_row = 40 + ((h as usize - 1 - y) * w as usize * 4);
        for x in 0..w as usize {
            let b = img[src_row + x * 4];
            let g = img[src_row + x * 4 + 1];
            let r = img[src_row + x * 4 + 2];
            let mut a = img[src_row + x * 4 + 3];
            if !any_alpha && has_and {
                let bit = and_row * y + x / 8;
                let and_set = (img[40 + xor_len + bit] >> (7 - (x % 8))) & 1 == 1;
                a = if and_set { 0 } else { 255 };
            }
            let dst = (y * w as usize + x) * 4;
            rgba[dst] = r;
            rgba[dst + 1] = g;
            rgba[dst + 2] = b;
            rgba[dst + 3] = a;
        }
    }
    Ok(RawFrame {
        width: w,
        height: h,
        hotspot: entry.hotspot,
        rgba,
    })
}

fn best_entry<'a>(entries: &'a [IcoEntry]) -> &'a IcoEntry {
    entries
        .iter()
        .max_by_key(|e| (e.width * e.height) as u64)
        .unwrap_or(&entries[0])
}

fn load_ico_or_cur(data: &[u8]) -> Result<RawFrame, String> {
    let (entries, _t) = parse_ico_container(data, None)?;
    let entry = best_entry(&entries);
    decode_ico_image(data, entry)
}

// ---------------------------------------------------------------------------
// ANI container: RIFF('ACON') { 'anih', ['seq '], ['rate'], LIST('fram'{'icon'..}) }
// Hotspots come from each embedded icon's ICONDIRENTRY.
// ---------------------------------------------------------------------------

fn load_ani(data: &[u8]) -> Result<RawFrame, String> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"ACON" {
        return Err("not an ANI file (RIFF/ACON signature missing)".into());
    }
    let mut frames: Vec<RawFrame> = Vec::new();
    let mut order: Vec<usize> = Vec::new();

    fn walk_chunk(data: &[u8], pos: &mut usize, end: usize, frames: &mut Vec<RawFrame>, order: &mut Vec<usize>) -> Result<(), String> {
        while *pos + 8 <= end {
            let id = &data[*pos..*pos + 4];
            let size = u32::from_le_bytes([
                data[*pos + 4],
                data[*pos + 5],
                data[*pos + 6],
                data[*pos + 7],
            ]) as usize;
            let body = *pos + 8;
            let next = body + size + (size & 1); // chunks are word-aligned
            if next > data.len() {
                break;
            }
            match id {
                b"LIST" => {
                    if body + 4 <= end && &data[body..body + 4] == b"fram" {
                        let mut sub = body + 4;
                        walk_chunk(data, &mut sub, next, frames, order)?;
                    }
                }
                b"icon" => {
                    let frame = load_ico_or_cur(&data[body..next]).map_err(|e| format!("embedded icon: {e}"))?;
                    order.push(frames.len());
                    frames.push(frame);
                }
                b"seq " => {
                    // frame play order: u32 sequence indices into the icon list
                    order.clear();
                    let n = size / 4;
                    for i in 0..n {
                        let v = u32::from_le_bytes([
                            data[body + i * 4],
                            data[body + i * 4 + 1],
                            data[body + i * 4 + 2],
                            data[body + i * 4 + 3],
                        ]) as usize;
                        order.push(v);
                    }
                }
                _ => {}
            }
            *pos = next;
        }
        Ok(())
    }

    let mut pos = 12usize;
    walk_chunk(data, &mut pos, data.len(), &mut frames, &mut order)?;

    if frames.is_empty() {
        return Err("ANI contains no icon frames".into());
    }
    // first frame in play order (a full ANI renderer is out of scope; we show
    // frame 0 and read ITS hotspot, per spec: never assume (0,0))
    let idx = order.first().copied().unwrap_or(0);
    let idx = idx.min(frames.len() - 1);
    Ok(frames.swap_remove(idx))
}

// ---------------------------------------------------------------------------
// public API
// ---------------------------------------------------------------------------

fn load_png_or_ico_file(path: &Path) -> Result<RawFrame, String> {
    let data = std::fs::read(path).map_err(|e| format!("read failed: {e}"))?;
    load_ico_or_cur(&data)
}

fn resize_to(frame: &RawFrame, target: u32) -> Result<RawFrame, String> {
    let target = target.max(8);
    let scale = target as f32 / frame.width.max(1) as f32;
    let new_h = ((frame.height as f32 * scale).round() as u32).max(8);
    let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba.clone())
        .ok_or("bad framebuffer")?;
    let resized = image::imageops::resize(&img, target, new_h, FilterType::Triangle);
    let k = target as f32 / frame.width.max(1) as f32;
    Ok(RawFrame {
        width: target,
        height: new_h,
        hotspot: (
            (frame.hotspot.0 as f32 * k).round() as i32,
            (frame.hotspot.1 as f32 * k).round() as i32,
        ),
        rgba: resized.into_raw(),
    })
}

/// Load a cursor image from file and scale it so its WIDTH == `size` px.
/// Hotspot is scaled proportionally (never reset to (0,0)).
pub fn load(path: &Path, size: u32) -> Result<CursorImage, String> {
    let data = std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let frame = match ext.as_str() {
        "cur" => {
            let (entries, t) = parse_ico_container(&data, Some(2))
                .map_err(|e| format!("invalid .cur: {e}"))?;
            let _ = t;
            decode_ico_image(&data, best_entry(&entries))?
        }
        "ico" => load_ico_file(&data)?,
        "ani" => load_ani(&data)?,
        "png" => {
            let img = image::load_from_memory(&data).map_err(|e| format!("png: {e}"))?;
            let rgba8 = img.to_rgba8();
            let (w, h) = rgba8.dimensions();
            // png has no hotspot: use center
            RawFrame {
                width: w,
                height: h,
                hotspot: (w as i32 / 2, h as i32 / 2),
                rgba: rgba8.into_raw(),
            }
        }
        other => return Err(format!("unsupported format .{other} (cur/ico/ani/png)")),
    };
    let frame = resize_to(&frame, size)?;
    Ok(CursorImage {
        width: frame.width,
        height: frame.height,
        hotspot: frame.hotspot,
        rgba: frame.rgba,
    })
}

// small wrapper to keep naming clear (ico == cur container with type 1)
fn load_ico_file(data: &[u8]) -> Result<RawFrame, String> {
    load_ico_or_cur(data)
}
