// Copyright 2016 Vernon Jones.
// Original code Copyright 2013 Manpreet Singh ( junkblocker@yahoo.com ). All rights reserved.
// Original code Copyright 2011 The Go Authors.  All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

use libcsearch::reader::{IndexReader, POST_ENTRY_SIZE};

use libprofiling;
use libvarint;

use std::u32;

#[derive(Debug)]
pub struct IdRange {
    pub low: u32,
    pub high: u32,
    pub new: u32,
}

pub struct PostMapReader<'a> {
    index: &'a IndexReader,
    pub id_map: Vec<IdRange>,
    tri_num: u32,
    pub trigram: u32,
    count: u32,
    offset: u32,
    d: &'a [u8],
    old_id: u32,
    pub file_id: u32,
    i: usize,
}

impl<'a> PostMapReader<'a> {
    pub fn new(index: &'a IndexReader, id_map: Vec<IdRange>) -> PostMapReader<'a> {
        let _frame = libprofiling::profile("PostMapReader::new");
        let s = unsafe { index.as_slice() };
        let mut p = PostMapReader {
            index: index,
            id_map: id_map,
            tri_num: 0,
            trigram: u32::MAX,
            count: 0,
            offset: 0,
            d: s,
            old_id: u32::MAX,
            file_id: 0,
            i: 0,
        };
        p.load();
        p
    }
    pub fn next_trigram(&mut self) {
        let _frame = libprofiling::profile("PostMapReader::next_trigram");
        self.tri_num += 1;
        self.load();
    }
    fn load(&mut self) {
        if self.tri_num >= (self.index.num_post as u32) {
            self.trigram = u32::MAX;
            self.count = 0;
            self.file_id = u32::MAX;
            return;
        }
        let (trigram, count, offset) = self
            .index
            .list_at((self.tri_num as usize) * POST_ENTRY_SIZE);
        self.trigram = trigram;
        self.count = count;
        self.offset = offset;
        if count == 0 {
            self.file_id = u32::MAX;
            return;
        }
        self.d = unsafe {
            let s = self.index.as_slice();
            let split_point = self.index.post_data + self.offset + 3;
            let (_, right_side) = s.split_at(split_point as usize);
            right_side
        };
        self.old_id = u32::MAX;
        self.i = 0;
    }
    pub fn next_id(&mut self) -> bool {
        while self.count > 0 {
            self.count -= 1;
            let (delta, n) = libvarint::read_uvarint(self.d).unwrap();
            if n <= 0 || delta == 0 {
                panic!("merge: inconsistent index at trigram {}", self.trigram);
            }
            self.d = self.d.split_at(n as usize).1;
            self.old_id = self.old_id.wrapping_add(delta as u32);
            while self.i < self.id_map.len() && self.id_map[self.i].high <= self.old_id {
                self.i += 1;
            }
            if self.i >= self.id_map.len() {
                self.count = 0;
                break;
            }
            if self.old_id < self.id_map[self.i].low {
                continue;
            }
            self.file_id = self.id_map[self.i].new + self.old_id - self.id_map[self.i].low;
            return true;
        }
        self.file_id = u32::MAX;
        return false;
    }
}
