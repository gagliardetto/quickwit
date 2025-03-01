// Copyright (C) 2021 Quickwit, Inc.
//
// Quickwit is offered under the AGPL v3.0 and as commercial software.
// For commercial licensing, contact us at hello@quickwit.io.
//
// AGPL:
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program. If not, see <http://www.gnu.org/licenses/>.

#![warn(missing_docs)]
#![allow(clippy::bool_assert_comparison)]

//! `quickwit-storage` is the abstraction used in quickwit to interface itself
//! to different storage:
//! - object storages (S3)
//! - local filesystem
//! - distributed filesystems.
//! etc.
//!
//! - The `BundleStorage` bundles together multiple files into a single file.
mod cache;
mod storage;
pub use self::storage::{PutPayload, Storage};

mod bundle_storage;
mod error;
mod local_file_storage;
mod object_storage;
mod prefix_storage;
mod ram_storage;
mod retry;
mod storage_resolver;
mod storage_with_upload_cache;

pub use storage_with_upload_cache::{
    create_storage_with_upload_cache, CacheParams, StorageWithUploadCache,
};

pub use self::bundle_storage::{
    BundleStorage, BundleStorageBuilder, BundleStorageFileOffsets, BUNDLE_FILENAME,
};
#[cfg(any(test, feature = "testsuite"))]
pub use self::cache::MockCache;
pub use self::local_file_storage::{LocalFileStorage, LocalFileStorageFactory};
pub use self::object_storage::{
    MultiPartPolicy, RegionProvider, S3CompatibleObjectStorage, S3CompatibleObjectStorageFactory,
};
pub use self::prefix_storage::add_prefix_to_storage;
pub use self::ram_storage::{RamStorage, RamStorageBuilder};
#[cfg(any(test, feature = "testsuite"))]
pub use self::storage::MockStorage;
#[cfg(any(test, feature = "testsuite"))]
pub use self::storage_resolver::MockStorageFactory;
pub use self::storage_resolver::{
    quickwit_storage_uri_resolver, StorageFactory, StorageUriResolver,
};
#[cfg(feature = "testsuite")]
pub use self::tests::storage_test_suite;
pub use crate::cache::{Cache, MemorySizedCache, SliceCache, StorageWithCacheFactory};
pub use crate::error::{StorageError, StorageErrorKind, StorageResolverError, StorageResult};

#[cfg(any(test, feature = "testsuite"))]
pub(crate) mod tests {

    use std::path::Path;

    use anyhow::Context;

    use crate::{PutPayload, Storage, StorageErrorKind};

    async fn test_get_inexistent_file(storage: &mut dyn Storage) -> anyhow::Result<()> {
        let err = storage
            .get_slice(Path::new("missingfile"), 0..3)
            .await
            .map_err(|err| err.kind());
        assert!(matches!(err, Err(StorageErrorKind::DoesNotExist)));
        Ok(())
    }

    async fn test_write_and_get_slice(storage: &mut dyn Storage) -> anyhow::Result<()> {
        let test_path = Path::new("write_and_read_slice");
        storage
            .put(
                test_path,
                PutPayload::from(&b"abcdefghiklmnopqrstuvxyz"[..]),
            )
            .await?;
        let payload = storage.get_slice(test_path, 3..6).await?;
        assert_eq!(&payload[..], b"def");
        Ok(())
    }

    async fn test_write_get_all(storage: &mut dyn Storage) -> anyhow::Result<()> {
        let test_path = Path::new("write_and_read_all");
        storage
            .put(test_path, PutPayload::from(&b"abcdef"[..]))
            .await?;
        let payload = storage.get_all(test_path).await?;
        assert_eq!(&payload[..], &b"abcdef"[..]);
        Ok(())
    }

    async fn test_write_and_cp(storage: &mut dyn Storage) -> anyhow::Result<()> {
        let test_path = Path::new("write_and_cp");
        let payload_bytes = b"abcdefghijklmnopqrstuvwxyz".as_ref();
        storage
            .put(test_path, PutPayload::from(payload_bytes))
            .await?;
        let tempdir = tempfile::tempdir()?;
        let dest_path = tempdir.path().to_path_buf();
        let local_copy = dest_path.join("local_copy");
        storage.copy_to_file(test_path, &local_copy).await?;
        let payload = std::fs::read(&local_copy)?;
        assert_eq!(&payload[..], payload_bytes);
        Ok(())
    }

    async fn test_write_and_delete(storage: &mut dyn Storage) -> anyhow::Result<()> {
        let test_path = Path::new("write_and_delete");
        let payload_bytes = b"abcdefghijklmnopqrstuvwxyz".as_ref();
        storage
            .put(test_path, PutPayload::from(payload_bytes))
            .await?;
        storage.delete(test_path).await?;
        let slice_err = storage
            .get_slice(test_path, 0..3)
            .await
            .map_err(|e| e.kind());
        assert!(matches!(slice_err, Err(StorageErrorKind::DoesNotExist)));
        Ok(())
    }

    async fn test_file_size(storage: &mut dyn Storage) -> anyhow::Result<()> {
        let test_path = Path::new("write_for_filesize");
        let payload_bytes = b"abcdefghijklmnopqrstuvwxyz".as_ref();
        storage
            .put(test_path, PutPayload::from(payload_bytes))
            .await?;
        assert_eq!(storage.file_num_bytes(test_path).await?, 26u64);
        storage.delete(test_path).await?;
        Ok(())
    }

    async fn test_exists(storage: &mut dyn Storage) -> anyhow::Result<()> {
        let test_path = Path::new("exists");
        assert!(matches!(storage.exists(test_path).await, Ok(false)));
        storage
            .put(test_path, PutPayload::from(b"".as_ref()))
            .await?;
        assert!(matches!(storage.exists(test_path).await, Ok(true)));
        assert!(matches!(storage.delete(test_path).await, Ok(())));
        Ok(())
    }

    async fn test_delete_missing_file(storage: &mut dyn Storage) -> anyhow::Result<()> {
        let test_path = Path::new("missing_file");
        assert!(matches!(storage.exists(test_path).await, Ok(false)));
        assert!(storage.delete(test_path).await.is_ok());
        Ok(())
    }

    async fn test_write_and_delete_with_dir_separator(
        storage: &mut dyn Storage,
    ) -> anyhow::Result<()> {
        let test_path = Path::new("foo/bar/write_and_delete_with_separator");
        let payload_bytes = b"abcdefghijklmnopqrstuvwxyz".as_ref();
        storage
            .put(test_path, PutPayload::from(payload_bytes))
            .await?;
        assert!(matches!(
            storage.exists(Path::new("foo/bar")).await,
            Ok(false)
        ));
        storage.delete(test_path).await?;

        assert!(matches!(
            storage.exists(Path::new("foo/bar")).await,
            Ok(false)
        ));
        assert!(matches!(storage.exists(Path::new("foo")).await, Ok(false)));
        Ok(())
    }

    /// Generic test suite for a storage.
    pub async fn storage_test_suite(storage: &mut dyn Storage) -> anyhow::Result<()> {
        test_get_inexistent_file(storage)
            .await
            .with_context(|| "get_inexistent_file")?;
        test_write_and_get_slice(storage)
            .await
            .with_context(|| "write_and_get_slice")?;
        test_write_get_all(storage)
            .await
            .with_context(|| "write_and_get_all")?;
        test_write_and_cp(storage)
            .await
            .with_context(|| "write_and_cp")?;
        test_write_and_delete(storage)
            .await
            .with_context(|| "write_and_delete")?;
        test_exists(storage).await.with_context(|| "exists")?;
        test_write_and_delete_with_dir_separator(storage)
            .await
            .with_context(|| "write_and_delete_with_separator")?;
        test_file_size(storage)
            .await
            .with_context(|| "delete_missing_file")?;
        test_delete_missing_file(storage)
            .await
            .with_context(|| "delete_missing_file")?;
        Ok(())
    }
}
