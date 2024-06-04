// Filesystem metadata struct
#![allow(dead_code)]

use libc::abs;

use crate::interface;
use super::syscalls::fs_constants::*;
use super::syscalls::sys_constants::*;
use super::net::NET_METADATA;

use super::cage::{Cage, FileDesc, FileDescriptor};

use std::io::BufRead;
use std::io::Read;
use std::io::*;

pub const METADATAFILENAME: &str = "lind.metadata";

pub const LOGFILENAME: &str = "lind.md.log";

pub static FS_METADATA: interface::RustLazyGlobal<interface::RustRfc<FilesystemMetadata>> = 
    interface::RustLazyGlobal::new(|| interface::RustRfc::new(FilesystemMetadata::init_fs_metadata())); //we want to check if fs exists before doing a blank init, but not for now

type FileObjectTable = interface::RustHashMap<usize, interface::EmulatedFile>;
pub static FILEOBJECTTABLE: interface::RustLazyGlobal<FileObjectTable> = 
    interface::RustLazyGlobal::new(|| interface::RustHashMap::new());

#[derive(interface::SerdeSerialize, interface::SerdeDeserialize, Debug,)]
pub enum Inode {
    File(GenericInode),
    CharDev(DeviceInode),
    Socket(SocketInode),
    Dir(DirectoryInode),
}

#[derive(interface::SerdeSerialize, interface::SerdeDeserialize, Debug)]
pub struct GenericInode {
    pub size: usize,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub linkcount: u32,
    #[serde(skip)] //skips serializing and deserializing field, will populate with u32 default of 0 (refcount should not be persisted)
    pub refcount: u32,
    pub atime: u64,
    pub ctime: u64,
    pub mtime: u64
}

#[derive(interface::SerdeSerialize, interface::SerdeDeserialize, Debug)]
pub struct DeviceInode {
    pub size: usize,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub linkcount: u32,
    #[serde(skip)] //skips serializing and deserializing field, will populate with u32 default of 0 (refcount should not be persisted)
    pub refcount: u32,
    pub atime: u64,
    pub ctime: u64,
    pub mtime: u64,
    pub dev: DevNo,
}

#[derive(interface::SerdeSerialize, interface::SerdeDeserialize, Debug)]
pub struct SocketInode {
    pub size: usize,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub linkcount: u32,
    #[serde(skip)]
    pub refcount: u32,
    pub atime: u64,
    pub ctime: u64,
    pub mtime: u64
}

#[derive(interface::SerdeSerialize, interface::SerdeDeserialize, Debug)]
pub struct DirectoryInode {
    pub size: usize,
    pub uid: u32,
    pub gid: u32,
    pub mode: u32,
    pub linkcount: u32,
    #[serde(skip)] //skips serializing and deserializing field, will populate with u32 default of 0 (refcount should not be persisted)
    pub refcount: u32,
    pub atime: u64,
    pub ctime: u64,
    pub mtime: u64,
    pub filename_to_inode_dict: interface::RustHashMap<String, usize>
}

#[derive(interface::SerdeSerialize, interface::SerdeDeserialize, Debug)]
pub struct FilesystemMetadata {
    pub nextinode: interface::RustAtomicUsize,
    pub dev_id: u64,
    /* A.W.: 
    *   [Wait for change]
    *   - Replace Inode with EmulatedFile?
    */
    pub inodetable: interface::RustHashMap<usize, Inode>
}

pub fn init_filename_to_inode_dict(curinode: usize, parentinode: usize) -> interface::RustHashMap<String, usize> {
    let retval = interface::RustHashMap::new();
    retval.insert(".".to_string(), curinode);
    retval.insert("..".to_string(), parentinode);
    retval
}

/* A.W.: 
*   Changes to remove usage of log-based fs
*/
impl FilesystemMetadata {

    pub fn blank_fs_init() -> FilesystemMetadata {
        //remove open files?
        let retval = FilesystemMetadata {nextinode: interface::RustAtomicUsize::new(STREAMINODE + 1), dev_id: 20, inodetable: interface::RustHashMap::new()};
        let time = interface::timestamp(); //We do a real timestamp now
        let dirinode = DirectoryInode {size: 0, uid: DEFAULT_UID, gid: DEFAULT_GID,
        //linkcount is how many entries the directory has (as per linux kernel), . and .. making 2 for the root directory initially,
        //plus one to make sure it can never be removed (can be thought of as mount point link)
        //refcount is how many open file descriptors pointing to the directory exist, 0 as no cages exist yet
            mode: S_IFDIR as u32 | S_IRWXA, linkcount: 3, refcount: 0,
            atime: time, ctime: time, mtime: time,
            filename_to_inode_dict: init_filename_to_inode_dict(ROOTDIRECTORYINODE, ROOTDIRECTORYINODE)};
        retval.inodetable.insert(ROOTDIRECTORYINODE, Inode::Dir(dirinode));

        retval
    }

    /* A.W.:
    *   Always initial - because we don't need persistent
    */
    pub fn init_fs_metadata() -> FilesystemMetadata {
        FilesystemMetadata::blank_fs_init()
    }
}

pub fn format_fs() {
    let newmetadata = FilesystemMetadata::blank_fs_init();
    //Because we keep the metadata as a synclazy, it is not possible to completely wipe it and
    //reinstate something over it in-place. Thus we create a new file system, wipe the old one, and 
    //then persist our new one. In order to create the new one, because the FS_METADATA does not
    //point to the same metadata that we are trying to create, we need to manually insert these
    //rather than using system calls.

    let mut rootinode = newmetadata.inodetable.get_mut(&1).unwrap(); //get root to populate its dict
    if let Inode::Dir(ref mut rootdir) = *rootinode {
        rootdir.filename_to_inode_dict.insert("dev".to_string(), 2);
        rootdir.linkcount += 1;
    } else {
        unreachable!();
    }
    drop(rootinode);

    let devchildren = interface::RustHashMap::new();
    devchildren.insert("..".to_string(), 1); 
    devchildren.insert(".".to_string(), 2); 
    devchildren.insert("null".to_string(), 3); 
    devchildren.insert("zero".to_string(), 4);
    devchildren.insert("urandom".to_string(), 5);
    devchildren.insert("random".to_string(), 6);

    let tmpchildren = interface::RustHashMap::new();
    tmpchildren.insert("..".to_string(), 1); 
    tmpchildren.insert(".".to_string(), 2); 

    let time = interface::timestamp(); //We do a real timestamp now
    let devdirinode = Inode::Dir(DirectoryInode {
        size: 0, uid: DEFAULT_UID, gid: DEFAULT_GID,
        mode: (S_IFDIR | 0755) as u32,
        linkcount: 3 + 4, //3 for ., .., and the parent dir, 4 is one for each child we will create
        refcount: 0,
        atime: time, ctime: time, mtime: time,
        filename_to_inode_dict: devchildren,
    }); //inode 2
    let nullinode = Inode::CharDev(DeviceInode {
        size: 0, uid: DEFAULT_UID, gid: DEFAULT_UID,
        mode: (S_IFCHR | 0666) as u32, linkcount: 1, refcount: 0,
        atime: time, ctime: time, mtime: time,
        dev: DevNo {major: 1, minor: 3},
    }); //inode 3
    let zeroinode = Inode::CharDev(DeviceInode {
        size: 0, uid: DEFAULT_UID, gid: DEFAULT_UID,
        mode: (S_IFCHR | 0666) as u32, linkcount: 1, refcount: 0,
        atime: time, ctime: time, mtime: time,
        dev: DevNo {major: 1, minor: 5},
    }); //inode 4
    let urandominode = Inode::CharDev(DeviceInode {
        size: 0, uid: DEFAULT_UID, gid: DEFAULT_UID,
        mode: (S_IFCHR | 0666) as u32, linkcount: 1, refcount: 0,
        atime: time, ctime: time, mtime: time,
        dev: DevNo {major: 1, minor: 9},
    }); //inode 5
    let randominode = Inode::CharDev(DeviceInode {
        size: 0, uid: DEFAULT_UID, gid: DEFAULT_UID,
        mode: (S_IFCHR | 0666) as u32, linkcount: 1, refcount: 0,
        atime: time, ctime: time, mtime: time,
        dev: DevNo {major: 1, minor: 8},
    }); //inode 6
    let tmpdirinode = Inode::Dir(DirectoryInode {
        size: 0, uid: DEFAULT_UID, gid: DEFAULT_GID,
        mode: (S_IFDIR | 0755) as u32,
        linkcount: 3 + 4, 
        refcount: 0,
        atime: time, ctime: time, mtime: time,
        filename_to_inode_dict: tmpchildren,
    }); //inode 7
    newmetadata.nextinode.store(8, interface::RustAtomicOrdering::Relaxed);
    newmetadata.inodetable.insert(2, devdirinode);
    newmetadata.inodetable.insert(3, nullinode);
    newmetadata.inodetable.insert(4, zeroinode);
    newmetadata.inodetable.insert(5, urandominode);
    newmetadata.inodetable.insert(6, randominode);
    newmetadata.inodetable.insert(7, tmpdirinode); 

    // let _logremove = interface::removefile(LOGFILENAME.to_string());

    // persist_metadata(&newmetadata);
}

/* A.W.:
*   2 kinds of readfile(): 
*       1) a txt file structured by (filename filesize filepath;filename2 filesize2 filepath2;...) 
*       2) a dir path that contains the actual file needs to be read, parameters: actual path = content_path + relative path 
*           (from input_path)
*/
fn create_missing_directory(path: &str, cageid: u64) -> std::io::Result<()> {
    // Recursively create parent directory if missing
    let mode = 0o777; // or any other default mode
    if path.len() == 0 { panic!("given path was null in loading phase"); }
    let cage = interface::cagetable_getref(cageid);
    let truepath = normpath(convpath(path), &cage);

    //pass the metadata to this helper. If passed table is none, then create new instance
    let metadata = &FS_METADATA;

    match metawalkandparent(truepath.as_path()) {
        //If neither the file nor parent exists
        (None, None) => {
            panic!("a directory component in pathname does not exist or is a dangling symbolic link in loading phase{:?}", truepath.as_path())
        }

        //If the file doesn't exist but the parent does
        (None, Some(pardirinode)) => {
            let filename = truepath.file_name().unwrap().to_str().unwrap().to_string(); //for now we assume this is sane, but maybe this should be checked later

            let effective_mode = S_IFDIR as u32 | mode;

            //assert sane mode bits
            if mode & (S_IRWXA | S_FILETYPEFLAGS as u32) != mode {
                panic!("Mode bits were not sane in loading phase");
            }

            let newinodenum = FS_METADATA.nextinode.fetch_add(1, interface::RustAtomicOrdering::Relaxed); //fetch_add returns the previous value, which is the inode number we want
            let time = interface::timestamp(); //We do a real timestamp now

            let newinode = Inode::Dir(DirectoryInode {
                size: 0, uid: DEFAULT_UID, gid: DEFAULT_GID,
                mode: effective_mode, linkcount: 3, refcount: 0, //2 because ., and .., as well as reference in parent directory
                atime: time, ctime: time, mtime: time, 
                filename_to_inode_dict: init_filename_to_inode_dict(newinodenum, pardirinode)
            });

            if let Inode::Dir(ref mut parentdir) = *(metadata.inodetable.get_mut(&pardirinode).unwrap()) {
                parentdir.filename_to_inode_dict.insert(filename, newinodenum);
                parentdir.linkcount += 1;
            } //insert a reference to the file in the parent directory
            else {unreachable!();}
            metadata.inodetable.insert(newinodenum, newinode);
            return Ok(()); 
        }

        (Some(_), ..) => {
            panic!("pathname already exists, cannot create directory {:?}",truepath.as_path());
        }
    }
}

pub fn load_fs(input_path: &str, content_path: &str, cageid: u64) -> std::io::Result<()> {
    // Open the loading file
    let file = interface::File::open(input_path)?;
    let mut reader = interface::BufReader::new(file);
    // Read file information entries (Read all bytes until a newline (the 0xA byte) is reached )
    let mut lines = String::new();
    let _ = reader.read_line(&mut lines);
    // Divide the file information into individual entries
    let mut file_entries = Vec::new();
    let entries = lines.split(';');
    for entry in entries {
        // Format: filename filesize filepath
        let parts: Vec<_> = entry.trim().split_whitespace().collect();
        if parts.len() == 3 {
            let filename = parts[0].to_string();
            let filesize = parts[1].parse::<usize>();
            let filepath = parts[2];
            if let Ok(filesize) = filesize {
                file_entries.push((filename, filesize, filepath));
            } else {
                eprintln!("Error parsing size for entry: {}", entry);
            }
        }
    }
   
    // Read contents into EmulatedFile according to file information entry
    for (filename, filesize, filepath) in file_entries {
        let abs_content_path = format!("{}{}", content_path, filepath);

        println!("Begin File - {:?}", abs_content_path);
        std::io::stdout().flush().unwrap();

        // Adding dir
        let cage = interface::cagetable_getref(cageid);
        let truepath = normpath(convpath(filepath), &cage);
        if filepath.len() == 0 { panic!("No path in loading phase"); }

        let mut ancestor = interface::RustPathBuf::from("/");
        for component in truepath.parent().unwrap().components() {
            ancestor.push(component);

            //Walk the file tree to get inode from path
            if let Some(inodenum) = metawalk(&ancestor) {
                let inodeobj = FS_METADATA.inodetable.get(&inodenum).unwrap();
                match &*inodeobj {
                    Inode::Dir(_f) => {
                        continue;
                    },
                    _ => {
                        break;
                    }
                }
            }
            let _ = create_missing_directory(ancestor.to_str().unwrap(), cageid);
        }

        println!("Line 338 File - {:?}", abs_content_path);
        std::io::stdout().flush().unwrap();

        // let mut contentfile = interface::File::open(abs_content_path)?;
        let mut contentfile = interface::File::open(abs_content_path.clone())?;
        let mut filedata = Vec::new();
        let _ = contentfile.read_to_end(&mut filedata)?;
        
        // Create a new emulated file and write the contents
        let mut emulated_file = interface::openfile(filename.clone()).unwrap();
        let _ = emulated_file.writefile_from_bytes(&filedata).unwrap();

        println!("Line 350 File - {:?}", abs_content_path);
        std::io::stdout().flush().unwrap();
        
        // println!("[DEBUG] \nname: {:?} \nfile size: {:?} \nmemory block{:?}", emulated_file.filename, emulated_file.filesize, emulated_file.memory_block);
        // std::io::stdout().flush().unwrap();

        // let (fd, guardopt) = cage.get_next_fd(None);
        // println!("[DEBUG] \nname: {:?} \nfile size: {:?} \nmemory block{:?}", emulated_file.filename, emulated_file.filesize, emulated_file.memory_block);
        // println!("fd: {:?}", fd);
        // std::io::stdout().flush().unwrap();
        // if fd < 0 { panic!("Cannot get fd table in loading phase"); }
        
        // let fdoption = &mut *guardopt.unwrap();
        let flags = O_RDWR | O_CREAT | O_APPEND;
        let mode = 0o777;
        let effective_mode = S_IFREG as u32 | mode;
        let fd = cage.open_syscall(&abs_content_path, flags, effective_mode);
        if fd < 0 {
            println!("fd: {:?}", fd);
            std::io::stdout().flush().unwrap();
            panic!("Open error in loading phase");
        }
        // match metawalkandparent(truepath.as_path()) {
        //     (None, None) => {
        //         panic!("Cannot create files in loading phase: {:?}", truepath.as_path());
        //     }
        //     (None, Some(pardirinode)) => {
        //         let mode = 0o777;
        //         let effective_mode = S_IFREG as u32 | mode;
        //         let time = interface::timestamp(); //We do a real timestamp now
        //         let newinode = Inode::File(GenericInode {
        //             size: filesize, uid: DEFAULT_UID, gid: DEFAULT_GID,
        //             mode: effective_mode, linkcount: 1, refcount: 1,
        //             atime: time, ctime: time, mtime: time,
        //         });
        //         let newinodenum = FS_METADATA.nextinode.fetch_add(1, interface::RustAtomicOrdering::Relaxed); //fetch_add returns the previous value, which is the inode number we want
        //         if let Inode::Dir(ref mut ind) = *(FS_METADATA.inodetable.get_mut(&pardirinode).unwrap()) {
        //             ind.filename_to_inode_dict.insert(filename.clone(), newinodenum);
        //             ind.linkcount += 1;
        //             //insert a reference to the file in the parent directory
        //         } else {
        //             panic!("It's a dictionary in loading phase");
        //         }
                
        //         FS_METADATA.inodetable.insert(newinodenum, newinode);
        //         if let interface::RustHashEntry::Vacant(vac) = FILEOBJECTTABLE.entry(newinodenum){
        //             let _ = emulated_file.close();
        //             vac.insert(emulated_file);
        //         }
                
        //         // Add to FDTABLE
        //         let position = if 0 != flags & O_APPEND {filesize} else {0};
        //         let allowmask = O_RDWRFLAGS | O_CLOEXEC;
        //         let newfd = FileDesc {position: position, inode: newinodenum, flags: flags & allowmask, advlock: interface::RustRfc::new(interface::AdvisoryLock::new())};
        //         let _insertval = fdoption.insert(FileDescriptor::File(newfd));
                
        //     }
        //     (Some(_inodenum), ..) => {
        //         // panic!("File already exists in loading phasae");
        //     }
        // }
        println!("Loaded File - {:?}", abs_content_path);
        std::io::stdout().flush().unwrap();
        cage.close_syscall(fd);
        println!("Closed File - {:?}", abs_content_path);
        std::io::stdout().flush().unwrap();
    }
    
    Ok(())
    
}



pub fn fsck() {
    FS_METADATA.inodetable.retain(|_inodenum, inode_obj| {
        match inode_obj {
            Inode::File(ref mut normalfile_inode) => {
                normalfile_inode.linkcount != 0
            },
            Inode::Dir(ref mut dir_inode) => {
                //2 because . and .. always contribute to the linkcount of a directory
                dir_inode.linkcount > 2
            },
            Inode::CharDev(ref mut char_inodej) => {
                char_inodej.linkcount != 0
            },
            Inode::Socket(_) => { false }
         }
    });
}


pub fn convpath(cpath: &str) -> interface::RustPathBuf {
    interface::RustPathBuf::from(cpath)
}

/// This function resolves the absolute path of a directory from its inode number in a filesystem. 
/// Here's how it operates:
///
/// - Starts from the given inode and fetches its associated metadata from the filesystem's inode table.
///
/// - Verifies that the inode represents a directory.
///
/// - Attempts to find the parent directory by looking for the ".." entry in the current directory's entries.
///
/// - Retrieves the directory name associated with the current inode using the `filenamefrominode` function and prepends it to the `path_string`.
///
/// - Continues this process recursively, updating the current inode to the parent inode, and accumulating directory names in the `path_string`.
///
/// - Stops when it reaches the root directory, where it prepends a "/" to the `path_string` and returns the complete path.
///
/// This function effectively constructs the absolute path by backtracking the parent directories. However, if any issues arise during this process, such as missing metadata or inability to find the parent directory, it returns `None`.
pub fn pathnamefrominodenum(inodenum: usize) -> Option<String>{
    let mut path_string = String::new();
    let mut first_iteration = true;
    let mut current_inodenum = inodenum;

    loop{
        let mut thisinode = match FS_METADATA.inodetable.get_mut(&current_inodenum) {
            Some(inode) => inode,
            None => {
                return None;
            },
        };

        match *thisinode {
            Inode::Dir(ref mut dir_inode) => {
                // We try to get the parent directory inode.
                if let Some(parent_dir_inode) = dir_inode.filename_to_inode_dict.get("..") {

                    // If the parent node is 1 (indicating the root directory) and this is not the first iteration, this indicates that we have arrived at the root directory. Here we add a '/' to the beginning of the path string and return it.
                    if *parent_dir_inode == (1 as usize){
                        if !first_iteration {
                            path_string.insert(0, '/');
                            return Some(path_string);
                        }
                        first_iteration = false;
                    }

                    match filenamefrominode(*parent_dir_inode, current_inodenum) {
                        Some(filename) => {
                            path_string = filename + "/" + &path_string;
                            current_inodenum = *parent_dir_inode;
                        },
                        None => return None,
                    };

                } else {
                    return None;
                }

            },
            _ => {
                return None;
            }
        }
    }
}


// Find the file by the given inode number in the given directory
pub fn filenamefrominode(dir_inode_no: usize, target_inode: usize) -> Option<String> {
    let cur_node = Some(FS_METADATA.inodetable.get(&dir_inode_no).unwrap());

    match &*cur_node.unwrap() {
        Inode::Dir(d) => {

            let mut target_variable_name: Option<String> = None;

            for entry in d.filename_to_inode_dict.iter() {
                if entry.value() == &target_inode {
                    target_variable_name = Some(entry.key().to_owned());
                    break;
                }
            }
            return target_variable_name;
        }
        _ => return None,
    }
}

//returns tuple consisting of inode number of file (if it exists), and inode number of parent (if it exists)
pub fn metawalkandparent(path: &interface::RustPath) -> (Option<usize>, Option<usize>) {

    let mut curnode = Some(FS_METADATA.inodetable.get(&ROOTDIRECTORYINODE).unwrap());
    let mut inodeno = Some(ROOTDIRECTORYINODE);
    let mut previnodeno = None;

    //Iterate over the components of the pathbuf in order to walk the file tree
    for comp in path.components() {
        match comp {
            //We've already done what initialization needs to be done
            interface::RustPathComponent::RootDir => {},

            interface::RustPathComponent::Normal(f) => {
                //If we're trying to get the child of a nonexistent directory, exit out
                if inodeno.is_none() {return (None, None);}
                match &*curnode.unwrap() { 
                    Inode::Dir(d) => {
                        previnodeno = inodeno;

                        //populate child inode number from parent directory's inode dict
                        inodeno = match d.filename_to_inode_dict.get(&f.to_str().unwrap().to_string()) {
                            Some(num) => {
                                curnode = FS_METADATA.inodetable.get(&num);
                                Some(*num)
                            }

                            //if no such child exists, update curnode, inodeno accordingly so that
                            //we can check against none as we do at the beginning of the Normal match arm
                            None => {
                                curnode = None;
                                None
                            }
                        }
                    }
                    //if we're trying to get a child of a non-directory inode, exit out
                    _ => {return (None, None);}
                }
            },

            //If it's a component of the pathbuf that we don't expect given a normed path, exit out
            _ => {return (None, None);}
        }
    }
    //return inode number and it's parent's number
    (inodeno, previnodeno)
}
pub fn metawalk(path: &interface::RustPath) -> Option<usize> {
    metawalkandparent(path).0
}
pub fn normpath(origp: interface::RustPathBuf, cage: &Cage) -> interface::RustPathBuf {
    //If path is relative, prefix it with the current working directory, otherwise populate it with rootdir
    let mut newp = if origp.is_relative() {(**cage.cwd.read()).clone()} else {interface::RustPathBuf::from("/")};

    for comp in origp.components() {
        match comp {
            //if we have a normal path component, push it on to our normed path
            interface::RustPathComponent::Normal(_) => {newp.push(comp);},

            //if we have a .. path component, pop the last component off our normed path
            interface::RustPathComponent::ParentDir => {newp.pop();},

            //if we have a . path component (Or a root dir or a prefix(?)) do nothing
            _ => {},
        };
    }
    newp
}

pub fn remove_domain_sock(truepath: interface::RustPathBuf) {
    match metawalkandparent(truepath.as_path()) {
        //If the file does not exist
        (None, ..) => { panic!("path does not exist") }
        //If the file exists but has no parent, it's the root directory
        (Some(_), None) => { panic!("cannot unlink root directory") }

        //If both the file and the parent directory exists
        (Some(inodenum), Some(parentinodenum)) => {
            Cage::remove_from_parent_dir(parentinodenum, &truepath);

            FS_METADATA.inodetable.remove(&inodenum);
            NET_METADATA.domsock_paths.remove(&truepath);
        }
    }
}

pub fn incref_root() {
    if let Inode::Dir(ref mut rootdir_dirinode_obj) = *(FS_METADATA.inodetable.get_mut(&ROOTDIRECTORYINODE).unwrap()) {
        rootdir_dirinode_obj.refcount += 1;
    } else {panic!("Root directory inode was not a directory");}
}

pub fn decref_dir(cwd_container: &interface::RustPathBuf) {
    if let Some(cwdinodenum) = metawalk(&cwd_container) {
        if let Inode::Dir(ref mut cwddir) = *(FS_METADATA.inodetable.get_mut(&cwdinodenum).unwrap()) {
            cwddir.refcount -= 1;

            //if the directory has been removed but this cwd was the last open handle to it
            if cwddir.refcount == 0 && cwddir.linkcount == 0 {
                FS_METADATA.inodetable.remove(&cwdinodenum);
            }
        } else {panic!("Cage had a cwd that was not a directory!");}
    } else {panic!("Cage had a cwd which did not exist!");}//we probably want to handle this case, maybe cwd should be an inode number?? Not urgent
}
