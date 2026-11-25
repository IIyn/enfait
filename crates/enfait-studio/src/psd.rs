//! Writing a layered Photoshop document (PSD, version 1, 8-bit RGB).
//!
//! Each layer is a raster with transparency, cropped to what it shows,
//! named in Unicode (`luni`); the merged picture follows, for the programs
//! that only read that. Channels are compressed with PackBits (RLE).
//!
//! Reference: Adobe Photoshop File Formats Specification.

use anyhow::{Result, ensure};
use image::RgbaImage;

/// A layer to write, at the size of the document.
pub struct PsdLayer<'a> {
    pub name: &'a str,
    pub image: &'a RgbaImage,
}

/// Largest side accepted by PSD version 1.
const MAX_SIDE: u32 = 30_000;

/// The document: `composite` (merged picture) and `layers` from the bottom
/// up, all of the same size.
pub fn write(composite: &RgbaImage, layers: &[PsdLayer<'_>]) -> Result<Vec<u8>> {
    let (width, height) = composite.dimensions();
    ensure!(
        width > 0 && height > 0 && width <= MAX_SIDE && height <= MAX_SIDE,
        "picture size not supported by PSD: {width}×{height}"
    );
    for layer in layers {
        ensure!(
            layer.image.dimensions() == (width, height),
            "layer {} is not the size of the picture",
            layer.name
        );
    }
    let mut out = Vec::new();
    // Header.
    out.extend_from_slice(b"8BPS");
    put_u16(&mut out, 1);
    out.extend_from_slice(&[0; 6]);
    put_u16(&mut out, 3);
    put_u32(&mut out, height);
    put_u32(&mut out, width);
    put_u16(&mut out, 8);
    put_u16(&mut out, 3); // RGB
    // Colour mode data, image resources: none.
    put_u32(&mut out, 0);
    put_u32(&mut out, 0);
    // Layers.
    let info = layer_info(layers)?;
    let mut section = Vec::new();
    put_u32(&mut section, len32(info.len())?);
    section.extend_from_slice(&info);
    put_u32(&mut section, 0); // global layer mask
    put_u32(&mut out, len32(section.len())?);
    out.extend_from_slice(&section);
    // Merged picture: RLE, row counts of every channel, then the rows.
    put_u16(&mut out, 1);
    let planes: Vec<Vec<Vec<u8>>> = (0..3)
        .map(|c| rows(composite, (0, 0, width, height), c))
        .collect();
    for plane in &planes {
        for row in plane {
            put_u16(&mut out, u16::try_from(row.len())?);
        }
    }
    for row in planes.iter().flatten() {
        out.extend_from_slice(row);
    }
    Ok(out)
}

/// Bounds `(left, top, right, bottom)` of the visible pixels, if any.
fn bounds(image: &RgbaImage) -> Option<(u32, u32, u32, u32)> {
    let (w, h) = image.dimensions();
    let (mut left, mut top, mut right, mut bottom) = (w, h, 0, 0);
    for (x, y, p) in image.enumerate_pixels() {
        if p.0[3] > 0 {
            left = left.min(x);
            top = top.min(y);
            right = right.max(x.saturating_add(1));
            bottom = bottom.max(y.saturating_add(1));
        }
    }
    (right > left && bottom > top).then_some((left, top, right, bottom))
}

/// Rows of channel `channel` (0 to 2: R, G, B; 3: alpha) in `rect`,
/// compressed.
fn rows(image: &RgbaImage, rect: (u32, u32, u32, u32), channel: usize) -> Vec<Vec<u8>> {
    let (left, top, right, bottom) = rect;
    (top..bottom)
        .map(|y| {
            let raw: Vec<u8> = (left..right)
                .map(|x| image.get_pixel(x, y).0.get(channel).copied().unwrap_or(0))
                .collect();
            packbits(&raw)
        })
        .collect()
}

fn layer_info(layers: &[PsdLayer<'_>]) -> Result<Vec<u8>> {
    let mut records = Vec::new();
    let mut data = Vec::new();
    put_u16(&mut records, u16::try_from(layers.len())?);
    for layer in layers {
        let rect = bounds(layer.image);
        // Channel data: alpha (-1), then R, G, B.
        let mut channels: Vec<(i16, Vec<u8>)> = Vec::with_capacity(4);
        for (id, c) in [(-1_i16, 3_usize), (0, 0), (1, 1), (2, 2)] {
            let mut bytes = Vec::new();
            match rect {
                Some(rect) => {
                    put_u16(&mut bytes, 1);
                    let rows = rows(layer.image, rect, c);
                    for row in &rows {
                        put_u16(&mut bytes, u16::try_from(row.len())?);
                    }
                    for row in &rows {
                        bytes.extend_from_slice(row);
                    }
                }
                None => put_u16(&mut bytes, 0),
            }
            channels.push((id, bytes));
        }
        let (left, top, right, bottom) = rect.unwrap_or((0, 0, 0, 0));
        for v in [top, left, bottom, right] {
            put_u32(&mut records, v);
        }
        put_u16(&mut records, 4);
        for (id, bytes) in &channels {
            records.extend_from_slice(&id.to_be_bytes());
            put_u32(&mut records, len32(bytes.len())?);
        }
        records.extend_from_slice(b"8BIMnorm");
        records.extend_from_slice(&[255, 0, 0, 0]); // opacity, clipping, flags, filler
        let mut extra = Vec::new();
        put_u32(&mut extra, 0); // layer mask
        put_u32(&mut extra, 0); // blending ranges
        extra.extend_from_slice(&pascal_name(layer.name));
        extra.extend_from_slice(&unicode_name(layer.name)?);
        put_u32(&mut records, len32(extra.len())?);
        records.extend_from_slice(&extra);
        for (_, bytes) in channels {
            data.extend_from_slice(&bytes);
        }
    }
    records.extend_from_slice(&data);
    if records.len() % 2 == 1 {
        records.push(0);
    }
    Ok(records)
}

/// Name as a Pascal string (MacRoman approximated by ASCII), padded to a
/// multiple of 4 bytes.
fn pascal_name(name: &str) -> Vec<u8> {
    let ascii: Vec<u8> = name
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() {
                c as u8
            } else {
                b'_'
            }
        })
        .take(255)
        .collect();
    let mut out = vec![u8::try_from(ascii.len()).unwrap_or(255)];
    out.extend_from_slice(&ascii);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

/// `luni` block: the name in UTF-16, what Photoshop shows.
fn unicode_name(name: &str) -> Result<Vec<u8>> {
    let units: Vec<u16> = name.encode_utf16().collect();
    let mut body = Vec::new();
    put_u32(&mut body, len32(units.len())?);
    for u in units {
        put_u16(&mut body, u);
    }
    while body.len() % 4 != 0 {
        body.push(0);
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"8BIMluni");
    put_u32(&mut out, len32(body.len())?);
    out.extend_from_slice(&body);
    Ok(out)
}

/// PackBits compression of a row.
fn packbits(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len().saturating_add(raw.len() / 128 + 1));
    let mut rest = raw;
    while let Some(&first) = rest.first() {
        // Run of identical bytes.
        let run = rest.iter().take(128).take_while(|&&b| b == first).count();
        if run >= 2 {
            out.push(u8::try_from(257 - run).unwrap_or(0));
            out.push(first);
            rest = rest.get(run..).unwrap_or_default();
            continue;
        }
        // Literal bytes, up to the next pair of identical bytes.
        let mut n = 1;
        while n < 128 && n < rest.len() {
            if matches!((rest.get(n), rest.get(n + 1)), (Some(a), Some(b)) if a == b) {
                break;
            }
            n += 1;
        }
        out.push(u8::try_from(n - 1).unwrap_or(0));
        out.extend_from_slice(rest.get(..n).unwrap_or_default());
        rest = rest.get(n..).unwrap_or_default();
    }
    out
}

fn put_u16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}

fn len32(len: usize) -> Result<u32> {
    Ok(u32::try_from(len)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn unpack(mut data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        while let Some((&n, rest)) = data.split_first() {
            if n < 128 {
                let k = usize::from(n) + 1;
                out.extend_from_slice(&rest[..k]);
                data = &rest[k..];
            } else if n > 128 {
                out.extend(std::iter::repeat_n(rest[0], 257 - usize::from(n)));
                data = &rest[1..];
            } else {
                data = rest;
            }
        }
        out
    }

    #[test]
    fn packbits_round_trip() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![7],
            vec![1, 2, 3, 3, 3, 3, 4, 5, 5],
            vec![9; 300],
            (0..=255).chain(0..=255).collect(),
        ];
        for raw in cases {
            assert_eq!(unpack(&packbits(&raw)), raw);
        }
        assert_eq!(packbits(&[9; 300]).len(), 6);
    }

    #[test]
    fn writes_a_layered_document() {
        let mut composite = RgbaImage::from_pixel(8, 4, Rgba([10, 20, 30, 255]));
        composite.put_pixel(2, 1, Rgba([255, 0, 0, 255]));
        let mut dot = RgbaImage::new(8, 4);
        dot.put_pixel(2, 1, Rgba([255, 0, 0, 255]));
        let empty = RgbaImage::new(8, 4);
        let psd = write(
            &composite,
            &[
                PsdLayer {
                    name: "Fond",
                    image: &composite,
                },
                PsdLayer {
                    name: "Titre é",
                    image: &dot,
                },
                PsdLayer {
                    name: "vide",
                    image: &empty,
                },
            ],
        )
        .unwrap();
        assert_eq!(&psd[..4], b"8BPS");
        assert_eq!(u32::from_be_bytes(psd[14..18].try_into().unwrap()), 4);
        assert_eq!(u32::from_be_bytes(psd[18..22].try_into().unwrap()), 8);
        assert_eq!(bounds(&dot), Some((2, 1, 3, 2)));
        assert_eq!(bounds(&empty), None);
        // The Unicode name is there.
        let utf16: Vec<u8> = "Titre é"
            .encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect();
        assert!(psd.windows(utf16.len()).any(|w| w == utf16.as_slice()));
        assert!(
            write(
                &composite,
                &[PsdLayer {
                    name: "x",
                    image: &RgbaImage::new(2, 2)
                }]
            )
            .is_err()
        );
    }
}
