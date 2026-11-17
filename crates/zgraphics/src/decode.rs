use alloc::sync::Arc;
use alloc::vec::Vec;
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;
use zune_inflate::{DeflateDecoder, DeflateOptions};
use zune_png::PngDecoder;

use crate::{Image, Limits, protocol::Command};

pub(crate) fn pixel_size(width: u32, height: u32, limits: Limits) -> Result<usize, &'static str> {
    if width == 0 || height == 0 || width > limits.max_dimension || height > limits.max_dimension {
        return Err("E2BIG:invalid or oversized image dimensions");
    }
    let size = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or("E2BIG:image dimensions overflow")?;
    if size > limits.max_image_bytes {
        return Err("E2BIG:image exceeds decoded size limit");
    }
    Ok(size)
}

pub(crate) fn base64_append(
    input: &[u8],
    out: &mut Vec<u8>,
    limit: usize,
) -> Result<(), &'static str> {
    let remainder = input.len() % 4;
    if input.len() > 4096 || remainder == 1 {
        return Err("EINVAL:invalid base64 length");
    }
    let padding = input.iter().rev().take_while(|b| **b == b'=').count();
    if padding > 2 || (padding != 0 && remainder != 0) {
        return Err("EINVAL:invalid base64 padding");
    }
    let size = input.len() / 4 * 3 + remainder.saturating_sub(1) - padding;
    if out.len().saturating_add(size) > limit {
        return Err("E2BIG:transfer exceeds memory limit");
    }
    // Reserve no more than the configured transfer cap, even under geometric
    // Vec growth. The scratch allocation is independent of image-store quota.
    if out.len() + size > out.capacity() {
        let wanted = (out.capacity().saturating_mul(2))
            .max(out.len() + size)
            .min(limit);
        out.try_reserve_exact(wanted - out.len())
            .map_err(|_| "ENOMEM:cannot allocate transfer")?;
    }
    let start = out.len();
    for (index, chunk) in input.as_chunks::<4>().0.iter().enumerate() {
        let last = index + 1 == input.len() / 4 && remainder == 0;
        let a = base64_value(chunk[0]);
        let b = base64_value(chunk[1]);
        let c = if chunk[2] == b'=' && last {
            Some(0)
        } else {
            base64_value(chunk[2])
        };
        let d = if chunk[3] == b'=' && last {
            Some(0)
        } else {
            base64_value(chunk[3])
        };
        let (Some(a), Some(b), Some(c), Some(d)) = (a, b, c, d) else {
            out.truncate(start);
            return Err("EINVAL:invalid base64 data");
        };
        if (chunk[2] == b'=' && chunk[3] != b'=')
            || (chunk[2] == b'=' && b & 15 != 0)
            || (chunk[3] == b'=' && chunk[2] != b'=' && c & 3 != 0)
        {
            out.truncate(start);
            return Err("EINVAL:invalid base64 padding bits");
        }
        out.push((a << 2) | (b >> 4));
        if chunk[2] != b'=' {
            out.push((b << 4) | (c >> 2));
        }
        if chunk[3] != b'=' {
            out.push((c << 6) | d);
        }
    }
    let tail = input.as_chunks::<4>().1;
    if !tail.is_empty() {
        let a = base64_value(tail[0]);
        let b = base64_value(tail[1]);
        let c = if tail.len() == 3 {
            base64_value(tail[2])
        } else {
            Some(0)
        };
        let (Some(a), Some(b), Some(c)) = (a, b, c) else {
            out.truncate(start);
            return Err("EINVAL:invalid unpadded base64 tail");
        };
        if (tail.len() == 2 && b & 15 != 0) || (tail.len() == 3 && c & 3 != 0) {
            out.truncate(start);
            return Err("EINVAL:invalid unpadded base64 bits");
        }
        out.push((a << 2) | (b >> 4));
        if tail.len() == 3 {
            out.push((b << 4) | (c >> 2));
        }
    }
    Ok(())
}

fn base64_value(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

pub(crate) fn image(
    c: &Command,
    mut bytes: Vec<u8>,
    limits: Limits,
) -> Result<Image, &'static str> {
    if c.compression == b'z' {
        let expected = if c.format == 100 {
            let n = c.size as usize;
            if n == 0 || n > limits.max_transfer_bytes {
                return Err("EINVAL:compressed PNG needs bounded S");
            }
            n
        } else {
            pixel_size(c.width, c.height, limits)? / 4 * (c.format as usize / 8)
        };
        let options = DeflateOptions::default()
            .set_limit(expected)
            .set_size_hint(expected)
            .set_confirm_checksum(true);
        bytes = DeflateDecoder::new_with_options(&bytes, options)
            .decode_zlib()
            .map_err(|_| "EINVAL:invalid or oversized zlib stream")?;
        if bytes.len() != expected {
            return Err("EINVAL:decompressed size mismatch");
        }
    }
    if c.format == 100 {
        return png(bytes, limits);
    }
    let size = pixel_size(c.width, c.height, limits)?;
    let expected = size / 4 * (c.format as usize / 8);
    if bytes.len() != expected {
        return Err("EINVAL:pixel data size mismatch");
    }
    let pixels = if c.format == 32 {
        bytes
    } else {
        let mut rgba = Vec::new();
        rgba.try_reserve_exact(size)
            .map_err(|_| "ENOMEM:cannot allocate image")?;
        for rgb in bytes.as_chunks::<3>().0 {
            rgba.extend_from_slice(rgb);
            rgba.push(255);
        }
        rgba
    };
    let mut pixels = pixels;
    pixels.shrink_to_fit();
    Ok(Image {
        width: c.width,
        height: c.height,
        pixels: Arc::new(pixels),
        generation: 0,
    })
}

fn png(mut bytes: Vec<u8>, limits: Limits) -> Result<Image, &'static str> {
    // Validate the complete container before the decoder can allocate anything.
    // Strip nonessential metadata in place: embedded ICC/text compression must
    // never turn an otherwise tiny terminal image into a metadata zip bomb.
    if bytes.len() < 33 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" {
        return Err("EINVAL:invalid PNG signature");
    }
    let mut read = 8usize;
    let mut write = 8usize;
    let mut dimensions = None;
    let mut ended = false;
    let mut idat = false;
    let mut idat_ended = false;
    let mut palette = false;
    let mut transparency = false;
    while read < bytes.len() {
        if bytes.len() - read < 12 {
            return Err("EINVAL:truncated PNG chunk");
        }
        let length = u32::from_be_bytes(bytes[read..read + 4].try_into().unwrap()) as usize;
        let end = read
            .checked_add(12)
            .and_then(|v| v.checked_add(length))
            .ok_or("EINVAL:PNG chunk overflow")?;
        if end > bytes.len() {
            return Err("EINVAL:truncated PNG payload");
        }
        let kind: [u8; 4] = bytes[read + 4..read + 8].try_into().unwrap();
        if !kind.iter().all(u8::is_ascii_alphabetic) || kind[2].is_ascii_lowercase() {
            return Err("EINVAL:invalid PNG chunk type");
        }
        let expected_crc = u32::from_be_bytes(bytes[end - 4..end].try_into().unwrap());
        if crc32(&bytes[read + 4..end - 4]) != expected_crc {
            return Err("EINVAL:PNG CRC mismatch");
        }
        let keep = match &kind {
            b"IHDR" => {
                if read != 8 || dimensions.is_some() || length != 13 {
                    return Err("EINVAL:invalid PNG header");
                }
                let width = u32::from_be_bytes(bytes[read + 8..read + 12].try_into().unwrap());
                let height = u32::from_be_bytes(bytes[read + 12..read + 16].try_into().unwrap());
                let _ = pixel_size(width, height, limits)?;
                dimensions = Some((width, height));
                true
            }
            b"PLTE" => {
                if dimensions.is_none()
                    || idat
                    || palette
                    || length == 0
                    || length > 768
                    || !length.is_multiple_of(3)
                {
                    return Err("EINVAL:invalid PNG palette");
                }
                palette = true;
                true
            }
            b"tRNS" => {
                if dimensions.is_none() || idat || transparency || length > 256 {
                    return Err("EINVAL:invalid PNG transparency");
                }
                transparency = true;
                true
            }
            b"IDAT" => {
                if dimensions.is_none() || idat_ended {
                    return Err("EINVAL:invalid PNG data order");
                }
                idat = true;
                true
            }
            b"IEND" => {
                if !idat || length != 0 || end != bytes.len() {
                    return Err("EINVAL:invalid PNG end");
                }
                ended = true;
                true
            }
            b"acTL" | b"fcTL" | b"fdAT" => return Err("ENOTSUP:animated PNG is not supported"),
            _ if kind[0].is_ascii_uppercase() => return Err("ENOTSUP:unknown critical PNG chunk"),
            _ => {
                if idat {
                    idat_ended = true;
                }
                false
            }
        };
        if keep {
            bytes.copy_within(read..end, write);
            write += end - read;
        }
        read = end;
    }
    if !ended {
        return Err("EINVAL:missing PNG end");
    }
    bytes.truncate(write);
    let (width, height) = dimensions.ok_or("EINVAL:missing PNG header")?;
    let size = pixel_size(width, height, limits)?;
    let options = DecoderOptions::new_safe()
        .set_max_width(limits.max_dimension as usize)
        .set_max_height(limits.max_dimension as usize)
        .set_strict_mode(true)
        .png_set_confirm_crc(true)
        .inflate_set_confirm_adler(true)
        .inflate_set_limit(size.saturating_mul(2).saturating_add(height as usize * 8))
        .png_set_add_alpha_channel(true)
        .png_set_strip_to_8bit(true)
        .png_set_decode_animated(false)
        .set_use_unsafe(false);
    let mut decoder = PngDecoder::new_with_options(ZCursor::new(&bytes), options);
    decoder
        .decode_headers()
        .map_err(|_| "EINVAL:invalid PNG header")?;
    if decoder.dimensions() != Some((width as usize, height as usize)) {
        return Err("EINVAL:PNG dimensions changed");
    }
    // 16-bit decoding uses a bounded two-byte intermediate before downsampling.
    if decoder.output_buffer_size().is_none_or(|n| n > size * 2) {
        return Err("E2BIG:PNG output exceeds limit");
    }
    let colorspace = decoder.colorspace();
    let data = decoder
        .decode_raw()
        .map_err(|_| "EINVAL:invalid PNG data")?;
    let pixels = match colorspace {
        Some(ColorSpace::RGBA) if data.len() == size => data,
        Some(ColorSpace::LumaA) if data.len() == size / 2 => {
            let mut rgba = Vec::new();
            rgba.try_reserve_exact(size)
                .map_err(|_| "ENOMEM:cannot allocate image")?;
            for pair in data.as_chunks::<2>().0 {
                rgba.extend_from_slice(&[pair[0], pair[0], pair[0], pair[1]]);
            }
            rgba
        }
        _ => return Err("EINVAL:unexpected PNG pixel layout"),
    };
    let mut pixels = pixels;
    pixels.shrink_to_fit();
    Ok(Image {
        width,
        height,
        pixels: Arc::new(pixels),
        generation: 0,
    })
}

fn crc32(bytes: &[u8]) -> u32 {
    const TABLE: [u32; 256] = {
        let mut table = [0; 256];
        let mut i = 0;
        while i < 256 {
            let mut value = i as u32;
            let mut bit = 0;
            while bit < 8 {
                value = (value >> 1) ^ (0xedb88320u32.wrapping_mul(value & 1));
                bit += 1;
            }
            table[i] = value;
            i += 1;
        }
        table
    };
    let mut crc = !0u32;
    for byte in bytes {
        crc = TABLE[((crc as u8) ^ byte) as usize] ^ (crc >> 8);
    }
    !crc
}
