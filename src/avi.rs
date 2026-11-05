//! Small MJPEG/AVI muxer. No REDCODE/R3D data is generated here.
//! RIFF layout: https://learn.microsoft.com/windows/win32/directshow/avi-riff-file-reference

use crate::display::Frame;
use image::{ExtendedColorType, codecs::jpeg::JpegEncoder};
use std::io::{self, Seek, SeekFrom, Write};

pub struct AviWriter<W: Write + Seek> {
    output: W,
    width: usize,
    height: usize,
    total_frames_offset: u64,
    stream_length_offset: u64,
    movi: u64,
    index: Vec<(u32, u32)>,
    rgb: Vec<u8>,
    finished: bool,
}

fn word(output: &mut impl Write, value: u32) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
}

fn patch(output: &mut (impl Write + Seek), position: u64, value: u32) -> io::Result<()> {
    let end = output.stream_position()?;
    output.seek(SeekFrom::Start(position))?;
    word(output, value)?;
    output.seek(SeekFrom::Start(end))?;
    Ok(())
}

pub fn jpeg(frame: &Frame, rgb: &mut Vec<u8>) -> io::Result<Vec<u8>> {
    if frame.width == 0
        || frame.height == 0
        || frame.width > 1920
        || frame.height > 1080
        || frame.rgba.len() != frame.width * frame.height * 4
    {
        return Err(io::Error::other("Invalid capture frame"));
    }
    rgb.clear();
    rgb.reserve(frame.width * frame.height * 3);
    for pixel in frame.rgba.chunks_exact(4) {
        rgb.extend_from_slice(&pixel[..3]);
    }
    let mut compressed = Vec::new();
    JpegEncoder::new_with_quality(&mut compressed, 85)
        .encode(
            rgb,
            frame.width as u32,
            frame.height as u32,
            ExtendedColorType::Rgb8,
        )
        .map_err(io::Error::other)?;
    Ok(compressed)
}

impl<W: Write + Seek> AviWriter<W> {
    pub fn new(mut output: W, width: usize, height: usize, fps: u32) -> io::Result<Self> {
        if width == 0 || height == 0 || width > 1920 || height > 1080 || !(1..=60).contains(&fps) {
            return Err(io::Error::other("Invalid AVI dimensions or frame rate"));
        }
        output.write_all(b"RIFF")?;
        word(&mut output, 0)?;
        output.write_all(b"AVI LIST")?;
        let hdrl = output.stream_position()?;
        word(&mut output, 0)?;
        output.write_all(b"hdrlavih")?;
        word(&mut output, 56)?;
        for value in [1_000_000 / fps, 0, 0, 0x10] {
            word(&mut output, value)?;
        }
        let total_frames_offset = output.stream_position()?;
        for value in [
            0,
            0,
            1,
            (width * height * 3) as u32,
            width as u32,
            height as u32,
            0,
            0,
            0,
            0,
        ] {
            word(&mut output, value)?;
        }
        output.write_all(b"LIST")?;
        let strl = output.stream_position()?;
        word(&mut output, 0)?;
        output.write_all(b"strlstrh")?;
        word(&mut output, 56)?;
        output.write_all(b"vidsMJPG")?;
        for value in [0, 0, 0, 1, fps, 0] {
            word(&mut output, value)?;
        }
        let stream_length_offset = output.stream_position()?;
        for value in [0, (width * height * 3) as u32, u32::MAX, 0] {
            word(&mut output, value)?;
        }
        for value in [0u16, 0, width as u16, height as u16] {
            output.write_all(&value.to_le_bytes())?;
        }
        output.write_all(b"strf")?;
        for value in [40, 40, width as u32, height as u32, 1 | (24 << 16)] {
            word(&mut output, value)?;
        }
        output.write_all(b"MJPG")?;
        for value in [(width * height * 3) as u32, 0, 0, 0, 0] {
            word(&mut output, value)?;
        }
        let end = output.stream_position()?;
        patch(&mut output, strl, (end - strl - 4) as u32)?;
        patch(&mut output, hdrl, (end - hdrl - 4) as u32)?;
        output.write_all(b"LIST")?;
        word(&mut output, 0)?;
        let movi = output.stream_position()?;
        output.write_all(b"movi")?;
        Ok(Self {
            output,
            width,
            height,
            total_frames_offset,
            stream_length_offset,
            movi,
            index: Vec::new(),
            rgb: Vec::new(),
            finished: false,
        })
    }

    pub fn frames(&self) -> u64 {
        self.index.len() as u64
    }

    pub fn bytes(&mut self) -> io::Result<u64> {
        self.output.stream_position()
    }

    pub fn push(&mut self, frame: &Frame) -> io::Result<()> {
        if self.finished || frame.width != self.width || frame.height != self.height {
            return Err(io::Error::other("AVI is closed or capture format changed"));
        }
        let encoded = jpeg(frame, &mut self.rgb)?;
        let position = self.output.stream_position()?;
        // Keep classic RIFF/AVI below 2 GiB, with room for the final index.
        if position + encoded.len() as u64 + (self.index.len() as u64 + 1) * 16 + 32
            >= 2_000_000_000
            || self.index.len() >= 100_000
        {
            return Err(io::Error::other(
                "Clip limit reached; start a new recording",
            ));
        }
        self.output.write_all(b"00dc")?;
        word(&mut self.output, encoded.len() as u32)?;
        self.output.write_all(&encoded)?;
        if encoded.len() % 2 != 0 {
            self.output.write_all(&[0])?;
        }
        self.index
            .push(((position - self.movi) as u32, encoded.len() as u32));
        Ok(())
    }

    pub fn finish(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        // Mark first so Drop cannot append a second index after a write failure.
        self.finished = true;
        let end_movi = self.output.stream_position()?;
        patch(
            &mut self.output,
            self.movi - 4,
            (end_movi - self.movi) as u32,
        )?;
        self.output.write_all(b"idx1")?;
        word(&mut self.output, (self.index.len() * 16) as u32)?;
        for (offset, size) in &self.index {
            self.output.write_all(b"00dc")?;
            for value in [0x10, *offset, *size] {
                word(&mut self.output, value)?;
            }
        }
        let end = self.output.stream_position()?;
        patch(&mut self.output, 4, (end - 8) as u32)?;
        patch(
            &mut self.output,
            self.total_frames_offset,
            self.index.len() as u32,
        )?;
        patch(
            &mut self.output,
            self.stream_length_offset,
            self.index.len() as u32,
        )?;
        self.output.flush()
    }
}

impl<W: Write + Seek> Drop for AviWriter<W> {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn clip_contains_decodable_frames_matching_its_index_and_duration() {
        let mut bytes = Cursor::new(Vec::new());
        {
            let mut writer = AviWriter::new(&mut bytes, 64, 32, 24).unwrap();
            for serial in 0..3 {
                writer
                    .push(&crate::capture::test_pattern(64, 32, serial))
                    .unwrap();
            }
            writer.finish().unwrap();
            writer.finish().unwrap();
            assert!(
                writer
                    .push(&crate::capture::test_pattern(64, 32, 4))
                    .is_err()
            );
        }
        let bytes = bytes.into_inner();
        let read =
            |offset| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(read(4), bytes.len() - 8);
        assert_eq!(read(48), 3);
        let movi = bytes.windows(4).position(|part| part == b"movi").unwrap();
        let index = bytes.windows(4).position(|part| part == b"idx1").unwrap();
        assert_eq!(read(index + 4), 48);
        for entry in 0..3 {
            let position = index + 8 + entry * 16;
            let chunk = movi + read(position + 8);
            let size = read(position + 12);
            assert_eq!(&bytes[chunk..chunk + 4], b"00dc");
            assert_eq!(read(chunk + 4), size);
            let image = image::load_from_memory(&bytes[chunk + 8..chunk + 8 + size]).unwrap();
            assert_eq!((image.width(), image.height()), (64, 32));
        }
    }
}
