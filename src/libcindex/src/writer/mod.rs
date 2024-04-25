// Copyright 2016 Vernon Jones. All rights reserved.
//
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};

pub use self::error::{IndexError, IndexErrorKind, IndexResult};
pub use self::write::IndexWriter;

mod error;
mod sparseset;
mod write;

mod postentry;
mod postheap;
mod postinglist;
mod sort_post;
mod trigramiter;

const NPOST: usize = (64 << 20) / 8; // 64 MB worth of post entries

/// Returns the offset in a seekable object.
pub fn get_offset<S: Seek>(seekable: &mut S) -> io::Result<u64> {
    seekable.seek(SeekFrom::Current(0))
}

/// Copies the data from a reader into a writer
pub fn copy_file<R: Read + Seek, W: Write>(dest: &mut BufWriter<W>, src: &mut R) {
    src.seek(SeekFrom::Start(0)).unwrap();
    let mut buf_src = BufReader::new(src);
    loop {
        let length = if let Ok(b) = buf_src.fill_buf() {
            if b.len() == 0 {
                break;
            }
            dest.write_all(b).unwrap();
            b.len()
        } else {
            break;
        };
        buf_src.consume(length);
    }
}

/// Used for writing trigrams
pub trait WriteTrigram: Write {
    /// Write a trigram to a stream
    ///
    /// Writes 24 bits of `t` into the stream
    fn write_trigram(&mut self, t: u32) -> io::Result<()> {
        let mut buf: [u8; 3] = [
            ((t >> 16) & 0xff) as u8,
            ((t >> 8) & 0xff) as u8,
            (t & 0xff) as u8,
        ];
        self.write_all(&mut buf)
    }
}

impl<W: Write + ?Sized> WriteTrigram for W {}
