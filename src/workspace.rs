// SPDX-FileCopyrightText: 2026 Chen Linxuan <me@black-desk.cn>
// SPDX-License-Identifier: GPL-3.0-or-later

//! Git worktree discovery and immutable disk snapshots, independent of LSP.
use crate::analysis::Document;
use std::{collections::HashMap, path::PathBuf, process::Command, sync::Arc};

#[derive(Default)]
pub struct Workspace {
    pub folders: Vec<PathBuf>,
    pub roots: Vec<PathBuf>,
    pub documents: HashMap<PathBuf, Arc<Document>>,
}

impl Workspace {
    /// Run on a blocking worker. Git supplies tracked and non-ignored untracked
    /// paths, including linked worktrees; submodules are not traversed.
    pub fn scan(folders: Vec<PathBuf>, mut previous: HashMap<PathBuf, Arc<Document>>) -> Self {
        let mut documents = HashMap::new();
        let mut roots = Vec::new();
        for folder in &folders {
            let output = Command::new("git")
                .arg("-C")
                .arg(folder)
                .args(["rev-parse", "--show-toplevel"])
                .output();
            let Ok(output) = output else { continue };
            if !output.status.success() {
                continue;
            }
            let Ok(root) = String::from_utf8(output.stdout) else {
                continue;
            };
            let root = PathBuf::from(root.trim_end_matches('\n'));
            if roots.contains(&root) {
                continue;
            }
            roots.push(root.clone());
            let output = Command::new("git")
                .arg("-C")
                .arg(&root)
                .args([
                    "ls-files",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                    "-z",
                ])
                .output();
            let Ok(output) = output else { continue };
            if !output.status.success() {
                continue;
            }
            for name in output.stdout.split(|&b| b == 0).filter(|s| !s.is_empty()) {
                let Ok(name) = std::str::from_utf8(name) else {
                    continue;
                };
                let path = root.join(name);
                if path.extension().is_none_or(|ext| ext != "dj") || documents.contains_key(&path) {
                    continue;
                }
                // Do not follow symlinks to files outside the repository.
                if !std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
                    continue;
                }
                match std::fs::read_to_string(&path) {
                    Ok(source) => {
                        let doc = previous
                            .remove(&path)
                            .filter(|doc| doc.source == source)
                            .unwrap_or_else(|| Arc::new(Document::new(source)));
                        documents.insert(path, doc);
                    }
                    Err(error) => log::warn!("Cannot index {}: {error}", path.display()),
                }
            }
        }
        Self {
            folders,
            roots,
            documents,
        }
    }
}
