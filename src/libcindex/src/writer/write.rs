// Original code Copyright 2011 The Go Authors.  All rights reserved.
// Original Code Copyright 2013 Manpreet Singh ( junkblocker@yahoo.com ). All rights reserved.
//
// Copyright 2016 Vernon Jones. All rights reserved.
//
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

#![allow(dead_code)]
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufWriter, Read, Write};
use std::mem;
use std::path::Path;

use byteorder::{BigEndian, WriteBytesExt};
use libprofiling;
use libvarint;
use tempfile::tempfile;

use consts::{MAGIC, TRAILER_MAGIC};

use super::error::{IndexError, IndexErrorKind, IndexResult};
use super::postentry::PostEntry;
use super::postheap::PostHeap;
use super::postinglist::{to_diffs, TakeWhilePeek};
use super::sort_post::sort_post;
use super::sparseset::SparseSet;
use super::trigramiter::TrigramReader;
use super::NPOST;
use super::{copy_file, get_offset, WriteTrigram};

// Index writing.  See read.rs for details of on-disk format.
//
// It would suffice to make a single large list of (trigram, file#) pairs
// while processing the files one at a time, sort that list by trigram,
// and then create the posting lists from subsequences of the list.
// However, we do not assume that the entire index fits in memory.
// Instead, we sort and flush the list to a new temporary file each time
// it reaches its maximum in-memory size, and then at the end we
// create the final posting lists by merging the temporary files as we
// read them back in.
//
// It would also be useful to be able to create an index for a subset
// of the files and then merge that index into an existing one.  This would
// allow incremental updating of an existing index when a directory changes.
// But we have not implemented that.

const MAX_FILE_LEN: u64 = 1 << 30;
const MAX_TEXT_TRIGRAMS: u64 = 30000;
const MAX_INVALID_UTF8_RATION: f64 = 0.1;
const MAX_LINE_LEN: u64 = 2000;

pub struct IndexWriter {
    /// Max number of allowed trigrams in a file
    pub max_trigram_count: u64,
    /// Max percentage of invalid utf-8 sequences allowed
    pub max_utf8_invalid: f64,
    /// Don't index a file if its size in bytes is larger than this
    pub max_file_len: u64,
    /// Stop indexing a file if it has a line longer than this
    pub max_line_len: u64,

    paths: Vec<OsString>,

    name_data: BufWriter<File>,
    name_index: BufWriter<File>,

    trigram: SparseSet,

    /// Tracks the number of names written to disk (used to assign file IDs)
    pub number_of_names_written: usize,
    /// Tracks the total number of bytes written to index
    pub bytes_written: usize,

    post: Vec<PostEntry>,
    post_files: Vec<Vec<PostEntry>>,
    post_index: BufWriter<File>,

    index: BufWriter<File>,
}

impl IndexWriter {
    /// Creates a new index file at `filename`
    ///
    /// ```no_run
    /// # use libcindex::writer::IndexWriter;
    /// let index = IndexWriter::new("index").unwrap();
    /// ```
    pub fn new<P: AsRef<Path>>(filename: P) -> io::Result<IndexWriter> {
        let _frame = libprofiling::profile("IndexWriter::new");
        let f = File::create(filename)?;
        Ok(IndexWriter {
            max_trigram_count: MAX_TEXT_TRIGRAMS,
            max_utf8_invalid: MAX_INVALID_UTF8_RATION,
            max_file_len: MAX_FILE_LEN,
            max_line_len: MAX_LINE_LEN,
            paths: Vec::new(),
            name_data: make_temp_buf()?,
            name_index: make_temp_buf()?,
            trigram: SparseSet::new(),
            number_of_names_written: 0,
            bytes_written: 0,
            post: Vec::with_capacity(NPOST),
            post_files: Vec::new(),
            post_index: make_temp_buf()?,
            index: BufWriter::with_capacity(256 << 10, f),
        })
    }

    /// Add the specified paths to the index.
    /// Note that this only writes the names of the paths into
    /// the index, it doesn't actually walk those directories.
    /// See `IndexWriter::add_file` for that.
    pub fn add_paths<I: IntoIterator<Item = OsString>>(&mut self, paths: I) {
        self.paths.extend(paths);
    }

    /// Open a file and index it
    ///
    /// ```no_run
    /// # use libcindex::writer::IndexWriter;
    /// let mut index = IndexWriter::new("index").unwrap();
    /// index.add_file("/path/to/file").unwrap();
    /// index.flush().unwrap();
    /// ```
    pub fn add_file<P: AsRef<Path>>(&mut self, filename: P) -> IndexResult<()> {
        let _frame = libprofiling::profile("IndexWriter::add_file");
        let f = File::open(filename.as_ref())?;
        let metadata = f.metadata()?;
        self.add(filename, f, metadata.len())
    }

    /// Indexes a file
    ///
    /// `filename` is the name of the opened file referred to by `f`.
    /// `size` is the size of the file referred to by `f`.
    pub fn add<P, R>(&mut self, filename: P, f: R, size: u64) -> IndexResult<()>
    where
        P: AsRef<Path>,
        R: Read,
    {
        let _frame = libprofiling::profile("IndexWriter::add");
        if size > self.max_file_len {
            return Err(IndexError::new(
                IndexErrorKind::FileTooLong,
                format!("file too long, ignoring ({} > {})", size, self.max_file_len),
            ));
        }
        self.trigram.clear();
        let max_utf8_invalid = ((size as f64) * self.max_utf8_invalid) as u64;
        {
            let mut trigrams = TrigramReader::new(f, max_utf8_invalid, self.max_line_len);
            let _trigram_insert_frame = libprofiling::profile("IndexWriter::add: Insert Trigrams");
            while let Some(each_trigram) = trigrams.next() {
                self.trigram.insert(each_trigram);
            }
            if let Some(e) = trigrams.take_error() {
                return e;
            }
        }
        if (self.trigram.len() as u64) > self.max_trigram_count {
            return Err(IndexError::new(
                IndexErrorKind::TooManyTrigrams,
                format!(
                    "Too many trigrams ({} > {})",
                    self.trigram.len(),
                    self.max_trigram_count
                ),
            ));
        }
        debug!("{} {} {:?}", size, self.trigram.len(), filename.as_ref());
        self.bytes_written += size as usize;

        let file_id = self.add_name(filename)?;
        let v = self.trigram.take_dense();
        self.push_trigrams_to_post(file_id, v)
    }

    /// Take trigrams in `trigams` and push them to the post list,
    /// possibly flushing them to file.
    fn push_trigrams_to_post(&mut self, file_id: u32, trigrams: Vec<u32>) -> IndexResult<()> {
        let _frame = libprofiling::profile("IndexWriter::push_trigrams_to_post");
        for each_trigram in trigrams {
            if self.post.len() >= NPOST {
                self.flush_post()?;
            }
            self.post.push(PostEntry::new(each_trigram, file_id));
        }
        Ok(())
    }

    /// Add `filename` to the nameData section of the index
    fn add_name<P: AsRef<Path>>(&mut self, filename: P) -> IndexResult<u32> {
        let _frame = libprofiling::profile("IndexWriter::add_name");
        let offset = get_offset(&mut self.name_data)?;
        self.name_index.write_u32::<BigEndian>(offset as u32)?;

        let s = filename.as_ref().to_str().ok_or(IndexError::new(
            IndexErrorKind::FileNameError,
            "UTF-8 Conversion error",
        ))?;
        self.name_data.write(s.as_bytes())?;
        self.name_data.write_u8(0)?;

        let id = self.number_of_names_written;
        self.number_of_names_written += 1;
        Ok(id as u32)
    }

    /// Finalize the index, collecting all data and writing it out.
    pub fn flush(mut self) -> IndexResult<()> {
        let _frame = libprofiling::profile("IndexWriter::flush");
        self.add_name("")?;
        self.index.write(MAGIC.as_bytes())?;

        let mut off = [0; 5];
        off[0] = get_offset(&mut self.index)?;

        for p in &self.paths {
            let path_as_bytes = p.to_str().map(str::as_bytes).ok_or(IndexError::new(
                IndexErrorKind::FileNameError,
                "UTF-8 Conversion error"
            ))?;
            self.index.write(path_as_bytes)?;
            self.index.write_u8(0)?;
        }
        self.index.write_u8(0)?;
        off[1] = get_offset(&mut self.index)?;

        self.name_data.flush()?;
        copy_file(&mut self.index, &mut self.name_data.get_mut());
        off[2] = get_offset(&mut self.index)?;

        self.merge_post()?;
        off[3] = get_offset(&mut self.index)?;

        self.name_index.flush()?;
        copy_file(&mut self.index, &mut self.name_index.get_mut());
        off[4] = get_offset(&mut self.index)?;

        self.post_index.flush()?;
        copy_file(&mut self.index, &mut self.post_index.get_mut());

        for v in off.iter() {
            self.index.write_u32::<BigEndian>(*v as u32)?;
        }
        self.index.write(TRAILER_MAGIC.as_bytes())?;
        info!(
            "{} data bytes, {} index bytes",
            self.bytes_written,
            get_offset(&mut self.index)?
        );
        Ok(())
    }
    /// Merge the posting lists together
    fn merge_post(&mut self) -> io::Result<()> {
        let _frame = libprofiling::profile("IndexWriter::merge_post");
        let mut heap = PostHeap::new();
        info!("merge {} files + mem", self.post_files.len());

        for p in self.post_files.drain(..) {
            heap.add_mem(p);
        }
        sort_post(&mut self.post);
        let mut v = Vec::new();
        mem::swap(&mut v, &mut self.post);
        heap.add_mem(v);

        let mut h = heap.into_iter().peekable();
        let offset0 = get_offset(&mut self.index)?;

        let _frame_write = libprofiling::profile(
            "IndexWriter::merge_post: Generate/Write post \
                                                  index",
        );
        while let Some(plist) = TakeWhilePeek::new(&mut h) {
            let _fname_write_to_index = libprofiling::profile(
                "IndexWriter::merge_post: Write \
                                                               post index",
            );
            let offset = get_offset(&mut self.index)? - offset0;

            // posting list
            let plist_trigram = plist.trigram();
            self.index.write_trigram(plist_trigram)?;
            let mut written = 0;
            let _fname_diffs = libprofiling::profile("IndexWriter::merge_post: Write file diffs");
            for each_file in to_diffs(plist.map(|p| p.file_id())) {
                libvarint::write_uvarint(&mut self.index, each_file)?;
                written += 1;
            }
            #[allow(dropping_copy_types)]
            drop(_fname_diffs);

            let _fname_diffs = libprofiling::profile("IndexWriter::merge_post: Write file diffs");
            self.post_index.write_trigram(plist_trigram)?;
            self.post_index.write_u32::<BigEndian>(written - 1)?;
            self.post_index.write_u32::<BigEndian>(offset as u32)?;
        }
        // NOTE: write last entry like how the go version works
        let offset = get_offset(&mut self.index)? - offset0;
        self.index.write_trigram(0xffffff)?; // END trigram
        libvarint::write_uvarint(&mut self.index, 0)?; // NUL byte for END postlist
        self.post_index.write_trigram(0xffffff)?; // END trigram
        self.post_index.write_u32::<BigEndian>(0)?; // nothing written
        self.post_index.write_u32::<BigEndian>(offset as u32)?;

        Ok(())
    }

    /// Flush the post data to a temporary file
    pub fn flush_post(&mut self) -> io::Result<()> {
        let _frame = libprofiling::profile("IndexWriter::flush_post");
        sort_post(&mut self.post);
        let mut v = Vec::with_capacity(NPOST);
        mem::swap(&mut v, &mut self.post);
        self.post_files.push(v);
        Ok(())
    }
}

fn make_temp_buf() -> io::Result<BufWriter<File>> {
    let w = tempfile()?;
    Ok(BufWriter::with_capacity(256 << 10, w))
}
