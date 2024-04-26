// Author: Nicholas Renner
//
// File related interface
#![allow(dead_code)]

use parking_lot::Mutex;
use std::sync::Arc;
use dashmap::DashSet;
pub use std::fs::{self, File, OpenOptions, canonicalize};
use std::env;
use std::slice;
pub use std::path::{PathBuf as RustPathBuf, Path as RustPath, Component as RustPathComponent};
pub use std::ffi::CStr as RustCStr;
pub use std::io::{SeekFrom, Seek, Read, Write, BufReader, BufWriter, Result};
pub use std::sync::{LazyLock as RustLazyGlobal, Mutex as RustMutex};
use std::ptr::copy;

use std::os::unix::io::{AsRawFd, RawFd};
// use libc::{mmap, mremap, munmap, PROT_READ, PROT_WRITE, MAP_SHARED, MREMAP_MAYMOVE, off64_t};
use std::ffi::c_void;
use std::convert::TryInto;
use crate::interface::errnos::{Errno, syscall_error};


pub static OPEN_FILES: RustLazyGlobal<Arc<DashSet<String>>> = RustLazyGlobal::new(|| Arc::new(DashSet::new()));



pub fn listfiles() -> Vec<String> {
    let paths = fs::read_dir(&RustPath::new(
        &env::current_dir().unwrap())).unwrap();
      
    let names =
    paths.filter_map(|entry| {
      entry.ok().and_then(|e|
        e.path().file_name()
        .and_then(|n| n.to_str().map(|s| String::from(s)))
      )
    }).collect::<Vec<String>>();

    return names;
}

// pub fn removefile(filename: String) -> std::io::Result<()> {
//     let openfiles = &OPEN_FILES;

//     if openfiles.contains(&filename) {
//         panic!("FileInUse");
//     }

//     let path: RustPathBuf = [".".to_string(), filename].iter().collect();

//     let absolute_filename = canonicalize(&path)?; //will return an error if the file does not exist

//     fs::remove_file(absolute_filename)?;

//     Ok(())
// }
fn is_allowed_char(c: char) -> bool{
    char::is_alphanumeric(c) || c == '.'
}

// Checker for illegal filenames
fn assert_is_allowed_filename(filename: &String) {

    const MAX_FILENAME_LENGTH: usize = 120;

    if filename.len() > MAX_FILENAME_LENGTH {
        panic!("ArgumentError: Filename exceeds maximum length.")
    }

    if !filename.chars().all(is_allowed_char) {
        println!("'{}'", filename);
        panic!("ArgumentError: Filename has disallowed characters.")
    }

    match filename.as_str() {
        "" | "." | ".." => panic!("ArgumentError: Illegal filename."),
        _ => {}
    }

    if filename.starts_with(".") {
        panic!("ArgumentError: Filename cannot start with a period.")
    }
}


#[derive(Debug)]
pub struct Memory {
    pub base_address: RustMutex<usize>,
    pub memory_list: RustMutex<Vec<usize>>,
}

// We want to Memory to be a global variable 
pub static GLOBAL_MEMORY: RustLazyGlobal<Memory> = RustLazyGlobal::new(|| {
    let page_size = 4096;
    // For test purpose
    let size = 1024 * 1024;
    
    let num_pages = if size % page_size == 0 {
        size / page_size
    } else {
        size / page_size + 1
    };

    Memory {
        base_address: RustMutex::new(0),
        memory_list: RustMutex::new(vec![0; num_pages]),
    }
});

pub fn allocate(request_size: usize) -> Vec<usize> {
    let memory_list_mutex = &GLOBAL_MEMORY.memory_list;
    let mut memorylist = memory_list_mutex.lock().unwrap();
    let page_size: usize = 4096; 
    // Compute number of pages we need
    let num_pages_needed = if request_size % page_size == 0 {
        request_size / page_size
    } else {
        request_size / page_size + 1
    };
    // Iterate memory list, allocate un-continous pages, and return index of new allocated page
    let mut allocated_block = Vec::new();
    for index in 0..memorylist.len() {
        if memorylist[index] == 0 {
            memorylist[index] = 1;
            allocated_block.push(index);
            if allocated_block.len() == num_pages_needed {
                return allocated_block;
            }
        }
    }
    // If there's no enough space for allocation, rollback assigned page tags
    if !allocated_block.is_empty() {
        for &index in &allocated_block {
            memorylist[index] = 0;
        }
    }
    panic!("No enough free pages available");
}

pub fn remove_fs(index_list: Vec<usize>) {
    let memory_list_mutex = &GLOBAL_MEMORY.memory_list;
    let mut memorylist = memory_list_mutex.lock().unwrap();
    for &index in &index_list {
        if index < memorylist.len() {
            memorylist[index] = 0;
        }
    }
}

#[derive(Debug)]
pub struct EmulatedFile {
    pub filesize: usize,
    pub memory_block: Vec<usize>,
    pub filename: String,
}

pub fn openfile(filename: String) -> std::io::Result<EmulatedFile> {
    EmulatedFile::new(filename)
}

impl EmulatedFile {

    fn new(filename: String) -> std::io::Result<EmulatedFile> {
        assert_is_allowed_filename(&filename);

        if OPEN_FILES.contains(&filename) {
            panic!("FileInUse");
        }
        OPEN_FILES.insert(filename.clone());
        Ok(EmulatedFile {filesize: 0 as usize, memory_block: Vec::new(), filename: filename})
    }

    pub fn open(&self) -> std::io::Result<()> {
        OPEN_FILES.insert(self.filename.clone());
        Ok(())
    }

    pub fn close(&self) -> std::io::Result<()> {
        OPEN_FILES.remove(&self.filename);
        Ok(())
    }

    /* A.W.:
    *   [Wait TODO]
    *   - fdatasync
    *   - fsync
    *   - sync_file_range
    */

    pub fn readat(&self, ptr: *mut u8, length: usize, offset: usize) -> std::io::Result<usize> {
        let mut ptr = ptr;
        let page_size = 4096;
        let _buf = unsafe {
            assert!(!ptr.is_null());
            slice::from_raw_parts_mut(ptr, length)
        };

        // Check for the maxium readable bytes
        let len; 
        if length > self.filesize - offset { len = self.filesize - offset; }
        else { len = length; }

        if offset > self.filesize {
            panic!("Seek offset extends past the EOF!");
        }
        // Calculate the offset
        // offset_block = start from which block
        // offset_pos = start from which position inside that block
        let (offset_block, offset_pos) = if offset / page_size == 0 {
            (0, offset)
        } else {
            (offset / page_size, offset % page_size)
        };
        let mut remain_len = len;
        for (i, &index) in self.memory_block.iter().enumerate() {
            if i < offset_block {
                // Skip blocks before starting
                continue;
            }
            let mem_base_addr_lock = &GLOBAL_MEMORY.base_address;
            match mem_base_addr_lock.lock() {
                Ok(mem_base_addr) => {
                    // Set ptr according to the start address for this block
                    let block_start = *mem_base_addr + page_size * index;
                    // Only consider offset in the first readable block
                    let ptr_mem: *mut u8 = (block_start + if i == offset_block { offset_pos } else { 0 }) as *mut u8;
                    // Calculate how many bytes need to be read this time
                    let bytes_to_copy = remain_len.min(page_size - if i == offset_block { offset_pos } else { 0 });
                    // Update remaining length
                    remain_len -= bytes_to_copy;
                    
                    unsafe {
                        copy(ptr_mem, ptr, bytes_to_copy);
                        ptr = ptr.add(bytes_to_copy);
                    }

                    if remain_len == 0 {
                        break;
                    }
                }
                Err(e) => {
                    panic!("Failed to acquire the lock in readat: {:?}", e);
                }
            }
        }
        
        Ok(len - remain_len)

    }

    // Write to file from provided C-buffer
    pub fn writeat(&mut self, ptr: *const u8, length: usize, offset: usize) -> std::io::Result<usize> {
        let mut ptr = ptr;
        let page_size = 4096;
        let _buf = unsafe {
            assert!(!ptr.is_null());
            slice::from_raw_parts(ptr, length)
        };

        if offset > self.filesize {
            panic!("Seek offset extends past the EOF!");
        }

        // Calculate the offset
        // offset_block = start from which block
        // offset_pos = start from which position inside that block
        let (offset_block, offset_pos) = if offset / page_size == 0 {
            (0, offset)
        } else {
            (offset / page_size, offset % page_size)
        };

        // Last block has enough space...?
        let lastblock_space = page_size - offset_pos;

        if self.memory_block.len() == 0 {
            // Initialization file memory
            self.filesize = length;
            let allocated = allocate(length);
            self.memory_block.extend(allocated.iter().cloned());
        } else if length + offset > lastblock_space {
            // If need extend
            let extendsize = length + offset - lastblock_space - self.filesize;
            self.filesize = length + offset;
            let extendblock = allocate(extendsize);
            self.memory_block.extend(extendblock.iter().cloned());
        } else { self.filesize = length + offset; }

        let mut remain_len = length;
        for (i, &index) in self.memory_block.iter().enumerate() {
            if i < offset_block {
                // Skip blocks before starting
                continue;
            }
            // Set ptr according to the start address for this block
            let mem_base_addr_lock = &GLOBAL_MEMORY.base_address;
            match mem_base_addr_lock.lock() {
                Ok(mem_base_addr) => {
                    let block_start = *mem_base_addr + page_size * index;
                    // Only consider offset in the first readable block
                    let ptr_mem: *mut u8 = (block_start + if i == offset_block { offset_pos } else { 0 }) as *mut u8;
                    // Calculate how many bytes need to be read this time
                    let bytes_to_copy = remain_len.min(page_size - if i == offset_block { offset_pos } else { 0 });
                    // Update remaining length
                    remain_len -= bytes_to_copy;
                    
                    unsafe {
                        copy(ptr, ptr_mem, bytes_to_copy);
                        ptr = ptr.add(bytes_to_copy);
                    }

                    if remain_len == 0 { break; }
                },
                Err(e) => {
                    panic!("Failed to acquire the lock in writeat: {:?}", e);
                }
            }
            
        }
        
        Ok(length - remain_len)

    }

    pub fn shrink(&mut self, length: usize) -> std::io::Result<()> {
        let page_size = 4096;
        if length > self.filesize { 
            panic!("Something is wrong. File is already smaller than length.");
        }
        // Find unused block: get the block and pos
        let new_block_total = if length / page_size == 0 {
            0
        } else {
            length / page_size + 1
        };
        let mut removed_block = Vec::new();
        // Update memory block
        if new_block_total + 1 < self.memory_block.len() {
            // Get the deleted block
            removed_block = self.memory_block.iter().skip(new_block_total + 1).cloned().collect();
            // self.memory_block.truncate(new_block_total + 1);
        }
        // Update memory list
        remove_fs(removed_block);
        // Update filesize
        self.filesize = length;         
        Ok(())
    }

    pub fn readfile_to_new_bytes(&self) -> std::io::Result<Vec<u8>> {
        // let mut stringbuf = Vec::new();
        let mut stringbuf = vec![0; self.filesize];
        self.readat(stringbuf.as_mut_ptr(), self.filesize, 0)?;
        Ok(stringbuf)
    }

    pub fn writefile_from_bytes(&mut self, buf: &[u8]) -> std::io::Result<()> {

        let length = buf.len();
        let offset = self.filesize;

        let ptr: *const u8 = buf.as_ptr();
    
        let _ = self.writeat(ptr, length, offset);

        if offset + length > self.filesize {
            self.filesize = offset + length;
        }
        
        Ok(())
    }

    pub fn zerofill_at(&mut self, offset: usize, count: usize) -> std::io::Result<usize> {
        let buf = vec![0; count];
        if offset > self.filesize {
            panic!("Seek offset extends past the EOF!");
        }
        let bytes_written = self.writeat(buf.as_ptr(), buf.len(), offset)?;

        if offset + count > self.filesize {
            self.filesize = offset + count;
        }

        Ok(bytes_written)
    }
 
}

/* A.W.: 
*   Commented
*/
// pub const COUNTMAPSIZE : usize = 8;
// pub const MAP_1MB : usize = usize::pow(2, 20);

// #[derive(Debug)]
// pub struct EmulatedFileMap {
//     filename: String,
//     abs_filename: RustPathBuf,
//     fobj: Arc<Mutex<File>>,
//     map: Arc<Mutex<Option<Vec<u8>>>>,
//     count: usize,
//     countmap:  Arc<Mutex<Option<Vec<u8>>>>,
//     mapsize: usize
// }

// pub fn mapfilenew(filename: String) -> std::io::Result<EmulatedFileMap> {
//     EmulatedFileMap::new(filename)
// }

// impl EmulatedFileMap {

//     fn new(filename: String) -> std::io::Result<EmulatedFileMap> {
//         // create new file like a normal emulated file, but always create
//         assert_is_allowed_filename(&filename);

//         let openfiles = &OPEN_FILES;

//         if openfiles.contains(&filename) {
//             panic!("FileInUse");
//         }

//         let path: RustPathBuf = [".".to_string(), filename.clone()].iter().collect();
//         let f = OpenOptions::new().read(true).write(true).create(true).open(filename.clone()).unwrap();
//         let absolute_filename = canonicalize(&path)?;
//         openfiles.insert(filename.clone());

//         let mapsize = MAP_1MB - COUNTMAPSIZE;   
//         // set the file equal to where were mapping the count and the actual map
//         let _newsize = f.set_len((COUNTMAPSIZE + mapsize) as u64).unwrap();

//         let map : Vec::<u8>;
//         let countmap : Vec::<u8>;

//         // here were going to map the first 8 bytes of the file as the "count" (amount of bytes written), and then map another 1MB for logging
//         unsafe {
//             let map_addr = mmap(0 as *mut c_void, MAP_1MB, PROT_READ | PROT_WRITE, MAP_SHARED, f.as_raw_fd() as i32, 0 as i64);
//             countmap =  Vec::<u8>::from_raw_parts(map_addr as *mut u8, COUNTMAPSIZE, COUNTMAPSIZE);
//             let map_ptr = map_addr as *mut u8;
//             map =  Vec::<u8>::from_raw_parts(map_ptr.offset(COUNTMAPSIZE as isize), mapsize, mapsize);
//         }
        
//         Ok(EmulatedFileMap {filename: filename, abs_filename: absolute_filename, fobj: Arc::new(Mutex::new(f)), map: Arc::new(Mutex::new(Some(map))), count: 0, countmap: Arc::new(Mutex::new(Some(countmap))), mapsize: mapsize})

//     }

//     pub fn write_to_map(&mut self, bytes_to_write: &[u8]) -> std::io::Result<()> {

//         let writelen = bytes_to_write.len();
        
//         // if we're writing past the current map, increase the map another 1MB
//         if writelen + self.count > self.mapsize {
//             self.extend_map();
//         }

//         let mut mapopt = self.map.lock();
//         let map = mapopt.as_deref_mut().unwrap();

//         let mapslice = &mut map[self.count..(self.count + writelen)];
//         mapslice.copy_from_slice(bytes_to_write);
//         self.count += writelen;
    

//         // update the bytes written in the map portion
//         let mut countmapopt = self.countmap.lock();
//         let countmap = countmapopt.as_deref_mut().unwrap();
//         countmap.copy_from_slice(&self.count.to_be_bytes());

//         Ok(())
//     }

//     fn extend_map(&mut self) {

//         // open count and map to resize mmap, and file to increase file size
//         let mut mapopt = self.map.lock();
//         let map = mapopt.take().unwrap();
//         let mut countmapopt = self.countmap.lock();
//         let countmap = countmapopt.take().unwrap();
//         let f = self.fobj.lock();

//         // add another 1MB to mapsize
//         let new_mapsize = self.mapsize + MAP_1MB;
//         let _newsize = f.set_len((COUNTMAPSIZE + new_mapsize) as u64).unwrap();

//         let newmap : Vec::<u8>;
//         let newcountmap : Vec::<u8>;

//         // destruct count and map and re-map
//         unsafe {
//             let (old_count_map_addr, countlen, _countcap) = countmap.into_raw_parts();
//             assert_eq!(COUNTMAPSIZE, countlen);
//             let (_old_map_addr, len, _cap) = map.into_raw_parts();
//             assert_eq!(self.mapsize, len);
//             let map_addr = mremap(old_count_map_addr as *mut c_void, COUNTMAPSIZE + self.mapsize, COUNTMAPSIZE + new_mapsize, MREMAP_MAYMOVE);

//             newcountmap =  Vec::<u8>::from_raw_parts(map_addr as *mut u8, COUNTMAPSIZE, COUNTMAPSIZE);
//             let map_ptr = map_addr as *mut u8;
//             newmap =  Vec::<u8>::from_raw_parts(map_ptr.offset(COUNTMAPSIZE as isize), new_mapsize, new_mapsize);
//         }

//         // replace maps
//         mapopt.replace(newmap);
//         countmapopt.replace(newcountmap);
//         self.mapsize = new_mapsize;
//     }

//     pub fn close(&self) -> std::io::Result<()> {
//         // remove file as open file and deconstruct map
//         let openfiles = &OPEN_FILES;
//         openfiles.remove(&self.filename);

//         let mut mapopt = self.map.lock();
//         let map = mapopt.take().unwrap();
//         let mut countmapopt = self.countmap.lock();
//         let countmap = countmapopt.take().unwrap();

//         unsafe {

//             let (countmap_addr, countlen, _countcap) = countmap.into_raw_parts();
//             assert_eq!(COUNTMAPSIZE, countlen);
//             munmap(countmap_addr as *mut c_void, COUNTMAPSIZE);

//             let (map_addr, len, _cap) = map.into_raw_parts();
//             assert_eq!(self.mapsize, len);
//             munmap(map_addr as *mut c_void, self.mapsize);
//         }
    
//         Ok(())
//     }
// }

#[derive(Debug)]
pub struct ShmFile {
    fobj: Arc<Mutex<File>>,
    key: i32,
    size: usize
}

pub fn new_shm_backing(key: i32, size: usize) -> std::io::Result<ShmFile> {
    ShmFile::new(key, size)
}

// Mimic shared memory in Linux by creating a file backing and truncating it to the segment size
// We can then safely unlink the file while still holding a descriptor to that segment,
// which we can use to map shared across cages.
impl ShmFile {
    fn new(key: i32, size: usize) -> std::io::Result<ShmFile> {

        // open file "shm-#id"
        let filename = format!("{}{}", "shm-", key);
        let f = OpenOptions::new().read(true).write(true).create(true).open(filename.clone()).unwrap();
        // truncate file to size
        f.set_len(size as u64)?;
        // unlink file
        fs::remove_file(filename)?;
        let shmfile = ShmFile {fobj: Arc::new(Mutex::new(f)), key: key, size: size};

        Ok(shmfile)
    }

    //gets the raw fd handle (integer) from a rust fileobject
    pub fn as_fd_handle_raw_int(&self) -> i32 {
        self.fobj.lock().as_raw_fd() as i32
    }
}

// convert a series of big endian bytes to a size
pub fn convert_bytes_to_size(bytes_to_write: &[u8]) -> usize {
    let sizearray : [u8; 8] = bytes_to_write.try_into().unwrap();
    usize::from_be_bytes(sizearray)
}

/* A.W.: 
*   Commented
*/
// #[cfg(test)]
// mod tests {
//     extern crate libc;
//     use std::mem;
//     use super::*;
//     #[test]
//     pub fn filewritetest() {
//       println!("{:?}", listfiles());
//       let mut f = openfile("foobar".to_string(), true).expect("?!");
//       println!("{:?}", listfiles());
//       let q = unsafe{libc::malloc(mem::size_of::<u8>() * 9) as *mut u8};
//       unsafe{std::ptr::copy_nonoverlapping("fizzbuzz!".as_bytes().as_ptr() , q as *mut u8, 9)};
//       println!("{:?}", f.writeat(q, 9, 0));
//       println!("fsync: {:?}", f.fsync().unwrap());
//       println!("fdatasync: {:?}", f.fdatasync().unwrap());
//       let b = unsafe{libc::malloc(mem::size_of::<u8>() * 9)} as *mut u8;
//       println!("{:?}", String::from_utf8(unsafe{std::slice::from_raw_parts(b, 9)}.to_vec()));
//       println!("{:?}", f.readat(b, 9, 0));
//       println!("{:?}", String::from_utf8(unsafe{std::slice::from_raw_parts(b, 9)}.to_vec()));
//       println!("{:?}", f.close());
//       unsafe {
//         libc::free(q as *mut libc::c_void);
//         libc::free(b as *mut libc::c_void);
//       }
//       println!("{:?}", removefile("foobar".to_string()));
//     }
// }

