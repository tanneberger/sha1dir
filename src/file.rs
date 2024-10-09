

use parking_lot::Mutex;
use sha1::{Digest, Sha1};

use rayon::{Scope, ThreadPoolBuilder};
use memmap::Mmap;
use std::env;

use std::fs::{self, File, Metadata};
use std::error::Error;
use std::fmt::{self, Display};
use std::io::{self, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Once;

type Result<T> = std::result::Result<T, Box<dyn Error>>;


pub fn die<P: AsRef<Path>, E: Display>(path: P, error: E) -> ! {
    static DIE: Once = Once::new();

    DIE.call_once(|| {
        let path = path.as_ref().display();
        let _ = writeln!(
            io::stderr(),
            "{}: {}",
            path,
            error,
        );
        process::exit(1);
    });

    unreachable!()
}

pub fn canonicalize<P: AsRef<Path>>(path: P) -> PathBuf {
    match fs::canonicalize(&path) {
        Ok(canonical) => canonical,
        Err(error) => die(path, error),
    }
}

pub fn checksum_current_dir(label: &Path, ignore_unknown_filetypes: bool) -> Checksum {
    let checksum = Checksum::new();
    rayon::scope(|scope| {
        if let Err(error) = (|| -> Result<()> {
            for child in Path::new(".").read_dir()? {
                let child = child?;
                scope.spawn({
                    let checksum = &checksum;
                    move |scope| {
                        entry(
                            scope,
                            label,
                            checksum,
                            Path::new(&child.file_name()),
                            ignore_unknown_filetypes,
                        );
                    }
                });
            }
            Ok(())
        })() {
            die(label, error);
        }
    });
    checksum
}



fn entry<'scope>(
    scope: &Scope<'scope>,
    base: &'scope Path,
    checksum: &'scope Checksum,
    path: &Path,
    ignore_unknown_filetypes: bool,
) {
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) => die(base.join(path), error),
    };

    let file_type = metadata.file_type();
    let result = if file_type.is_file() {
        file(checksum, path, metadata)
    } else if file_type.is_symlink() {
        symlink(checksum, path, metadata)
    } else if file_type.is_dir() {
        dir(
            scope,
            base,
            checksum,
            path,
            ignore_unknown_filetypes,
            metadata,
        )
    } else if file_type.is_socket() {
        socket(checksum, path, metadata)
    } else if ignore_unknown_filetypes {
        Ok(())
    } else {
        die(base.join(path), "Unsupported file type");
    };

    if let Err(error) = result {
        die(base.join(path), error);
    }
}

fn file(checksum: &Checksum, path: &Path, metadata: Metadata) -> Result<()> {
    let mut sha = begin(path, &metadata, b'f');

    // Enforced by memmap: "memory map must have a non-zero length"
    if metadata.len() > 0 {
        let file = File::open(path)?;
        let mmap = unsafe { Mmap::map(&file)? };
        sha.update(&mmap);
    }

    checksum.put(sha);

    Ok(())
}


fn dir<'scope>(
    scope: &Scope<'scope>,
    base: &'scope Path,
    checksum: &'scope Checksum,
    path: &Path,
    ignore_unknown_filetypes: bool,
    metadata: Metadata,
) -> Result<()> {
    let sha = begin(path, &metadata, b'd');
    checksum.put(sha);

    for child in path.read_dir()? {
        let child = child?.path();
        scope.spawn(move |scope| entry(scope, base, checksum, &child, ignore_unknown_filetypes));
    }

    Ok(())
}
