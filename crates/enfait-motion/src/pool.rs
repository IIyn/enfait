//! Several independent Chromium instances to render in parallel.
//!
//! Several tabs of one Chromium in "frame control" mode crash the
//! browser: each parallel task therefore has its own process (and its own
//! profile).

use crate::error::MotionError;
use crate::render::{Gpu, Renderer};
use std::path::Path;
use std::sync::Mutex;
use tokio::sync::Semaphore;

pub struct RendererPool {
    renderers: Vec<Renderer>,
    free: Mutex<Vec<usize>>,
    available: Semaphore,
}

/// A Chromium borrowed from the pool, returned at the end of the scope.
pub struct Lease<'a> {
    pool: &'a RendererPool,
    index: usize,
}

impl std::ops::Deref for Lease<'_> {
    type Target = Renderer;

    fn deref(&self) -> &Renderer {
        // The index comes from `free`, which only holds valid indices.
        #[allow(clippy::indexing_slicing)]
        &self.pool.renderers[self.index]
    }
}

impl Drop for Lease<'_> {
    fn drop(&mut self) {
        if let Ok(mut free) = self.pool.free.lock() {
            free.push(self.index);
        }
        self.pool.available.add_permits(1);
    }
}

impl RendererPool {
    /// Starts `size` Chromium instances, profiles in `profile_root/0`, `profile_root/1`…
    pub async fn launch(
        executable: &Path,
        profile_root: &Path,
        gpu: Gpu,
        size: usize,
    ) -> Result<Self, MotionError> {
        let size = size.max(1);
        let mut renderers = Vec::with_capacity(size);
        for i in 0..size {
            renderers
                .push(Renderer::launch(executable, &profile_root.join(i.to_string()), gpu).await?);
        }
        Ok(Self {
            renderers,
            free: Mutex::new((0..size).collect()),
            available: Semaphore::new(size),
        })
    }

    /// Waits for a free Chromium.
    pub async fn acquire(&self) -> Result<Lease<'_>, MotionError> {
        let permit = self
            .available
            .acquire()
            .await
            .map_err(|e| MotionError::Browser(e.to_string()))?;
        permit.forget();
        let index = self
            .free
            .lock()
            .ok()
            .and_then(|mut free| free.pop())
            .ok_or_else(|| MotionError::Browser("inconsistent render pool".into()))?;
        Ok(Lease { pool: self, index })
    }

    pub async fn close(self) {
        for renderer in self.renderers {
            renderer.close().await;
        }
    }
}
