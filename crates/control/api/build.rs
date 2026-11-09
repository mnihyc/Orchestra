//! Bundle the same public help articles used by the browser for in-process docs reads.
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

fn walk(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            walk(root, &path, files)?;
        } else if kind.is_file() && path.extension().is_some_and(|extension| extension == "md") {
            files.push(
                path.strip_prefix(root)
                    .map_err(io::Error::other)?
                    .to_path_buf(),
            );
        }
    }
    Ok(())
}

fn main() -> io::Result<()> {
    let manifest = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR")
            .ok_or_else(|| io::Error::other("missing manifest directory"))?,
    );
    let root = manifest.join("../../../docs/user");
    println!("cargo:rerun-if-changed={}", root.display());
    let mut files = Vec::new();
    walk(&root, &root, &mut files)?;
    files.sort();
    let mut output = String::from("pub(crate) const HELP: &[(&str,&str)] = &[\n");
    for file in files {
        let name = file.to_string_lossy().replace('\\', "/");
        let text = fs::read_to_string(root.join(file))?;
        output.push_str(&format!("({name:?},{text:?}),\n"));
    }
    output.push_str("];\n");
    let directory = PathBuf::from(
        env::var_os("OUT_DIR").ok_or_else(|| io::Error::other("missing output directory"))?,
    );
    fs::write(directory.join("ask_help.rs"), output)
}
