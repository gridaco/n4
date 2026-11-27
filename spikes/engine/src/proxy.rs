//! An explicit draft decode route. The authored shot still identifies its original asset.
//! Preview workers alone install these private indices. Inspection and export open originals.
use crate::{FrameRate, SourceProfiles, media::IndexedVideo};
use anyhow::{Context, Result, ensure};
use std::{collections::HashMap, path::PathBuf, sync::Arc};

/// Keys are the resolved original asset paths in the project. Values are immutable derivatives.
pub type ProxyMap = HashMap<PathBuf, PathBuf>;

pub(crate) fn install(
    sources: &mut SourceProfiles,
    proxies: &ProxyMap,
    fps: FrameRate,
) -> Result<()> {
    for (original, file) in proxies {
        ensure!(
            sources.keys().any(|(path, _)| path == original),
            "no project source for proxy key {}",
            original.display()
        );
        ensure!(
            original.canonicalize()? != file.canonicalize()?,
            "proxy is the original file"
        );
        for ((path, size), source) in sources.iter_mut() {
            if path == original {
                let proxy = IndexedVideo::open(file, size[0], size[1], fps)
                    .with_context(|| format!("open proxy {}", file.display()))?;
                source.check_proxy_timeline(&proxy).with_context(|| {
                    format!(
                        "proxy {} cannot stand in for {}",
                        file.display(),
                        original.display()
                    )
                })?;
                *source = Arc::new(proxy);
            }
        }
    }
    Ok(())
}
