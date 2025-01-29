use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug)]
struct GgufFile {
    path: PathBuf,
    size: u64,
}

impl GgufFile {
    fn new(path: PathBuf) -> Result<Self, std::io::Error> {
        let metadata = fs::metadata(&path)?;
        Ok(GgufFile {
            path,
            size: metadata.len(),
        })
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Starting from the root directory
    let root_dir = Path::new("/");

    // Walk the directory tree
    let gguf_files: Vec<GgufFile> = WalkDir::new(root_dir)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            if let Some(ext) = entry.path().extension() {
                ext == "gguf"
            } else {
                false
            }
        })
        .map(|entry| GgufFile::new(entry.into_path()))
        .collect::<Result<_, _>>()?;

    // Sort by size in descending order
    let mut sorted_files = gguf_files;
    sorted_files.sort_by(|a, b| b.size.cmp(&a.size));

    // Print the results
    for file in sorted_files {
        println!("Size: {} bytes\tPath: {}", file.size, file.path.display());
    }

    Ok(())
}
