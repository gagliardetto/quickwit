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

use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use quickwit_index_config::IndexConfig;
use quickwit_metastore::Metastore;
use quickwit_proto::{
    FetchDocsRequest, FetchDocsResponse, LeafSearchRequest, LeafSearchResponse,
    LeafSearchStreamRequest, LeafSearchStreamResult, SearchRequest, SearchResponse,
    SearchStreamRequest,
};
use quickwit_storage::StorageUriResolver;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::info;

use crate::search_stream::{leaf_search_stream, root_search_stream};
use crate::{fetch_docs, leaf_search, root_search, ClusterClient, SearchClientPool, SearchError};

#[derive(Clone)]
/// The search service implementation.
pub struct SearchServiceImpl {
    metastore: Arc<dyn Metastore>,
    storage_resolver: StorageUriResolver,
    cluster_client: ClusterClient,
    client_pool: Arc<SearchClientPool>,
}

/// Trait representing a search service.
///
/// It mirrors the gRPC service `SearchService`, but with a more concrete
/// error type that can be converted into an API Error.
/// The REST API relies directly on the `SearchService`.
/// Also, it is mockable.
#[mockall::automock]
#[async_trait]
pub trait SearchService: 'static + Send + Sync {
    /// Root search API.
    /// This RPC identifies the set of splits on which the query should run on,
    /// and dispatches the multiple calls to `LeafSearch`.
    ///
    /// It is also in charge of merging back the responses.
    async fn root_search(&self, request: SearchRequest) -> crate::Result<SearchResponse>;

    /// Performs a leaf search on a given set of splits.
    ///
    /// It is like a regular search except that:
    /// - the node should perform the search locally instead of dispatching
    /// it to other nodes.
    /// - it should be applied on the given subset of splits
    /// - hit content is not fetched, and we instead return a so-called `PartialHit`.
    async fn leaf_search(&self, request: LeafSearchRequest) -> crate::Result<LeafSearchResponse>;

    /// Fetches the documents contents from the document store.
    /// This methods takes `PartialHit`s and returns `Hit`s.
    async fn fetch_docs(&self, request: FetchDocsRequest) -> crate::Result<FetchDocsResponse>;

    /// Performs a root search returning a receiver for streaming
    async fn root_search_stream(&self, request: SearchStreamRequest) -> crate::Result<Vec<Bytes>>;

    /// Performs a leaf search on a given set of splits and returns a stream.
    async fn leaf_search_stream(
        &self,
        request: LeafSearchStreamRequest,
    ) -> crate::Result<UnboundedReceiverStream<crate::Result<LeafSearchStreamResult>>>;
}

impl SearchServiceImpl {
    /// Creates a new search service.
    pub fn new(
        metastore: Arc<dyn Metastore>,
        storage_resolver: StorageUriResolver,
        cluster_client: ClusterClient,
        client_pool: Arc<SearchClientPool>,
    ) -> Self {
        SearchServiceImpl {
            metastore,
            storage_resolver,
            cluster_client,
            client_pool,
        }
    }
}

fn deserialize_index_config(index_config_str: &str) -> crate::Result<Arc<dyn IndexConfig>> {
    let index_config =
        serde_json::from_str::<Arc<dyn IndexConfig>>(index_config_str).map_err(|err| {
            SearchError::InternalError(format!(
                "Failed to deserialize index config: `{}`",
                err.to_string()
            ))
        })?;
    Ok(index_config)
}

#[async_trait]
impl SearchService for SearchServiceImpl {
    async fn root_search(&self, search_request: SearchRequest) -> crate::Result<SearchResponse> {
        let search_result = root_search(
            &search_request,
            self.metastore.as_ref(),
            &self.cluster_client,
            &self.client_pool,
        )
        .await?;

        Ok(search_result)
    }

    async fn leaf_search(
        &self,
        leaf_search_request: LeafSearchRequest,
    ) -> crate::Result<LeafSearchResponse> {
        let search_request = leaf_search_request
            .search_request
            .ok_or_else(|| SearchError::InternalError("No search request.".to_string()))?;
        info!(index=?search_request.index_id, splits=?leaf_search_request.split_metadata, "leaf_search");
        let storage = self
            .storage_resolver
            .resolve(&leaf_search_request.index_uri)?;
        let split_ids = leaf_search_request.split_metadata;
        let index_config = deserialize_index_config(&leaf_search_request.index_config)?;

        let leaf_search_response = leaf_search(
            &search_request,
            storage.clone(),
            &split_ids[..],
            index_config,
        )
        .await?;

        Ok(leaf_search_response)
    }

    async fn fetch_docs(
        &self,
        fetch_docs_request: FetchDocsRequest,
    ) -> crate::Result<FetchDocsResponse> {
        let storage = self
            .storage_resolver
            .resolve(&fetch_docs_request.index_uri)?;

        let fetch_docs_response = fetch_docs(
            fetch_docs_request.partial_hits,
            storage,
            &fetch_docs_request.split_metadata,
        )
        .await?;

        Ok(fetch_docs_response)
    }

    async fn root_search_stream(
        &self,
        stream_request: SearchStreamRequest,
    ) -> crate::Result<Vec<Bytes>> {
        let data = root_search_stream(
            &stream_request,
            self.metastore.as_ref(),
            &self.cluster_client,
            &self.client_pool,
        )
        .await?;
        Ok(data)
    }

    async fn leaf_search_stream(
        &self,
        leaf_stream_request: LeafSearchStreamRequest,
    ) -> crate::Result<UnboundedReceiverStream<crate::Result<LeafSearchStreamResult>>> {
        let stream_request = leaf_stream_request
            .request
            .ok_or_else(|| SearchError::InternalError("No search request.".to_string()))?;
        info!(index=?stream_request.index_id, splits=?leaf_stream_request.split_metadata, "leaf_search");
        let storage = self
            .storage_resolver
            .resolve(&leaf_stream_request.index_uri)?;
        let index_config = deserialize_index_config(&leaf_stream_request.index_config)?;
        let leaf_receiver = leaf_search_stream(
            stream_request,
            storage.clone(),
            leaf_stream_request.split_metadata,
            index_config,
        )
        .await;
        Ok(leaf_receiver)
    }
}
