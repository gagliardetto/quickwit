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

#![allow(clippy::bool_assert_comparison)]

mod helpers;

use std::io::Read;
use std::path::Path;

use anyhow::Result;
use helpers::{TestEnv, TestStorageType};
use predicates::prelude::*;
use quickwit_cli::{create_index_cli, CreateIndexArgs};
use quickwit_metastore::{Metastore, MetastoreUriResolver, SplitState};
use serde_json::{Number, Value};
use serial_test::serial;
use tokio::time::{sleep, Duration};

use crate::helpers::{create_test_env, make_command, spawn_command};

fn create_logs_index(test_env: &TestEnv, index_id: &str) {
    make_command(
        format!(
            "new --index-uri {} --index-config-path {} --metastore-uri {}",
            test_env.index_uri(index_id),
            test_env.resource_files["config"].display(),
            test_env.metastore_uri,
        )
        .as_str(),
    )
    .assert()
    .success();
}

fn index_data(index_id: &str, input_path: &Path, metastore_uri: &str) {
    make_command(
        format!(
            "index --index-id {} --input-path {} --metastore-uri {}",
            index_id,
            input_path.display(),
            metastore_uri,
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("Indexed"))
    .stdout(predicate::str::contains("documents in"))
    .stdout(predicate::str::contains(
        "You can now query your index with",
    ));
}

#[test]
fn test_cmd_help() -> anyhow::Result<()> {
    let mut cmd = make_command("--help");
    cmd.assert()
        .success()
        .stdout(predicate::str::contains("USAGE"));
    Ok(())
}

#[tokio::test]
async fn test_cmd_new() -> Result<()> {
    let test_env = create_test_env(TestStorageType::LocalFileSystem)?;
    let index_id = "my-index";
    create_logs_index(&test_env, index_id);
    let index_metadata = test_env.metastore().index_metadata(index_id).await;

    assert_eq!(index_metadata.is_ok(), true);

    // Attempt to create with ill-formed new command.
    make_command("new")
        .assert()
        .failure()
        .stderr(predicate::str::contains("--index-uri <INDEX URI>"));

    Ok(())
}

#[test]
fn test_cmd_new_on_existing_index() -> Result<()> {
    let test_env = create_test_env(TestStorageType::LocalFileSystem)?;
    let index_id = "my-index";
    create_logs_index(&test_env, index_id);

    make_command(
        format!(
            "new --index-uri {} --index-config-path {} --metastore-uri {}",
            test_env.index_uri(index_id),
            test_env.resource_files["config"].display(),
            test_env.metastore_uri,
        )
        .as_str(),
    )
    .assert()
    .failure()
    .stderr(predicate::str::contains("already exists"));

    Ok(())
}

#[test]
fn test_cmd_index_on_non_existing_index() -> Result<()> {
    let test_env = create_test_env(TestStorageType::LocalFileSystem)?;
    make_command(
        format!(
            "index --index-id {} --input-path {} --metastore-uri {}",
            "non-existing-index",
            test_env.resource_files["logs"].display(),
            test_env.metastore_uri,
        )
        .as_str(),
    )
    .assert()
    .failure()
    .stderr(predicate::str::contains("does not exist"));

    Ok(())
}

#[test]
fn test_cmd_index_on_non_existing_file() -> Result<()> {
    let test_env = create_test_env(TestStorageType::LocalFileSystem)?;
    let index_id = "my-index";
    create_logs_index(&test_env, index_id);
    make_command(
        format!(
            "index --index-id {} --input-path {} --metastore-uri {}",
            index_id,
            test_env
                .local_directory_path
                .join("non-existing-data.json")
                .display(),
            test_env.metastore_uri,
        )
        .as_str(),
    )
    .assert()
    .failure()
    .stderr(predicate::str::contains("Command failed"));

    Ok(())
}

#[test]
fn test_cmd_index_simple() -> Result<()> {
    let test_env = create_test_env(TestStorageType::LocalFileSystem)?;
    let index_id = "my-index";
    create_logs_index(&test_env, index_id);

    index_data(
        index_id,
        test_env.resource_files["logs"].as_path(),
        &test_env.metastore_uri,
    );

    // using piped input.
    let log_path = test_env.resource_files["logs"].clone();
    make_command(
        format!(
            "index --index-id {} --metastore-uri {} ",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .pipe_stdin(log_path)?
    .assert()
    .success()
    .stdout(predicate::str::contains("Indexed"))
    .stdout(predicate::str::contains("documents in"))
    .stdout(predicate::str::contains(
        "You can now query your index with",
    ));

    Ok(())
}

#[test]
fn test_cmd_search() -> Result<()> {
    let test_env = create_test_env(TestStorageType::LocalFileSystem)?;
    let index_id = "my-index";
    create_logs_index(&test_env, index_id);

    index_data(
        index_id,
        test_env.resource_files["logs"].as_path(),
        &test_env.metastore_uri,
    );

    make_command(
        format!(
            "search --metastore-uri {} --index-id {} --query level:info",
            test_env.metastore_uri, index_id,
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::function(|output: &[u8]| {
        let result: Value = serde_json::from_slice(output).unwrap();
        result["numHits"] == Value::Number(Number::from(2i64))
    }));

    // search with tags
    make_command(
        format!(
            "search --metastore-uri {} --index-id {} --query level:info --tags city:paris \
             device:rpi",
            test_env.metastore_uri, index_id,
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::function(|output: &[u8]| {
        let result: Value = serde_json::from_slice(output).unwrap();
        result["numHits"] == Value::Number(Number::from(2i64))
    }));

    make_command(
        format!(
            "search --metastore-uri {} --index-id {} --query level:info --tags city:conakry",
            test_env.metastore_uri, index_id,
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::function(|output: &[u8]| {
        let result: Value = serde_json::from_slice(output).unwrap();
        result["numHits"] == Value::Number(Number::from(0i64))
    }));

    Ok(())
}

#[test]
fn test_cmd_delete_index_dry_run() -> Result<()> {
    let test_env = create_test_env(TestStorageType::LocalFileSystem)?;
    let index_id = "my-index";
    create_logs_index(&test_env, index_id);

    // Empty index.
    make_command(
        format!(
            "delete --index-id {} --metastore-uri {} --dry-run",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains("Only the index will be deleted"));

    index_data(
        index_id,
        test_env.resource_files["logs"].as_path(),
        &test_env.metastore_uri,
    );

    // Non-empty index
    make_command(
        format!(
            "delete --index-id {} --metastore-uri {} --dry-run",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "The following files will be removed",
    ))
    .stdout(predicate::str::contains(".split"));

    Ok(())
}

#[tokio::test]
async fn test_cmd_delete() -> Result<()> {
    let test_env = create_test_env(TestStorageType::LocalFileSystem)?;
    let index_id = "my-index";
    create_logs_index(&test_env, index_id);

    index_data(
        index_id,
        test_env.resource_files["logs"].as_path(),
        &test_env.metastore_uri,
    );

    make_command(
        format!(
            "gc --index-id {} --metastore-uri {}",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "No dangling files to garbage collect",
    ));

    make_command(
        format!(
            "delete --index-id {} --metastore-uri {}",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success();
    assert_eq!(
        test_env.metastore().index_metadata(index_id).await.is_ok(),
        false
    );

    Ok(())
}

#[tokio::test]
async fn test_cmd_garbage_collect_no_grace() -> Result<()> {
    let test_env = create_test_env(TestStorageType::LocalFileSystem)?;
    let index_id = "my-index";
    create_logs_index(&test_env, index_id);
    index_data(
        index_id,
        test_env.resource_files["logs"].as_path(),
        &test_env.metastore_uri,
    );

    let metastore = MetastoreUriResolver::default()
        .resolve(&test_env.metastore_uri)
        .await?;
    let splits = metastore.list_all_splits(index_id).await?;
    assert_eq!(splits.len(), 1);
    make_command(
        format!(
            "gc --index-id {} --metastore-uri {}",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "No dangling files to garbage collect",
    ));

    let index_path = test_env.local_directory_path.join(index_id);

    assert_eq!(index_path.exists(), true);

    let split_ids = &[splits[0].split_metadata.split_id.as_str()];
    metastore
        .mark_splits_for_deletion(index_id, split_ids)
        .await?;
    make_command(
        format!(
            "gc --index-id {} --metastore-uri {} --dry-run --grace-period 10m",
            index_id, test_env.metastore_uri,
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "The following files will be garbage collected.",
    ))
    .stdout(predicate::str::contains(".split"));

    for split_id in split_ids {
        let split_file = quickwit_common::split_file(split_id);
        let split_filepath = index_path.join(&split_file);
        assert_eq!(split_filepath.exists(), true);
    }

    make_command(
        format!(
            "gc --index-id {} --metastore-uri {} --grace-period 10m",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "Index `my-index` successfully garbage collected",
    ));

    for split_id in split_ids {
        let split_file = quickwit_common::split_file(split_id);
        let split_filepath = index_path.join(&split_file);
        assert_eq!(split_filepath.exists(), false);
    }

    let metastore = MetastoreUriResolver::default()
        .resolve(&test_env.metastore_uri)
        .await?;
    assert_eq!(metastore.list_all_splits(index_id).await?.len(), 0);

    make_command(
        format!(
            "delete --index-id {} --metastore-uri {}",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success();
    assert_eq!(index_path.exists(), false);
    Ok(())
}

#[tokio::test]
async fn test_cmd_garbage_collect_spares_files_within_grace_period() -> Result<()> {
    let test_env = create_test_env(TestStorageType::LocalFileSystem)?;
    let index_id = "my-index";
    create_logs_index(&test_env, index_id);
    index_data(
        index_id,
        test_env.resource_files["logs"].as_path(),
        &test_env.metastore_uri,
    );

    let metastore = test_env.metastore();
    let splits = metastore.list_all_splits(index_id).await?;
    assert_eq!(splits.len(), 1);
    make_command(
        format!(
            "gc --index-id {} --metastore-uri {}",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "No dangling files to garbage collect",
    ));

    let index_path = test_env.local_directory_path.join(index_id);
    let split_filename = quickwit_common::split_file(splits[0].split_metadata.split_id.as_str());
    let split_path = index_path.join(&split_filename);
    assert_eq!(split_path.exists(), true);

    // The following steps help turn an existing published split into a staged one
    // without deleting the files.
    let split_ids = vec![splits[0].split_metadata.split_id.as_str()];
    metastore
        .mark_splits_for_deletion(index_id, &split_ids)
        .await?;
    metastore.delete_splits(index_id, &split_ids).await?;
    let mut meta = splits[0].clone();
    meta.split_metadata.split_state = SplitState::New;
    metastore.stage_split(index_id, meta).await?;
    assert_eq!(split_path.exists(), true);

    make_command(
        format!(
            "gc --index-id {} --metastore-uri {} --grace-period 2s",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "No dangling files to garbage collect",
    ));
    assert_eq!(split_path.exists(), true);

    // wait for grace period
    sleep(Duration::from_secs(3)).await;
    make_command(
        format!(
            "gc --index-id {} --metastore-uri {} --dry-run --grace-period 2s",
            index_id, test_env.metastore_uri,
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "The following files will be garbage collected.",
    ))
    .stdout(predicate::str::contains(&split_filename));
    assert_eq!(split_path.exists(), true);

    make_command(
        format!(
            "gc --index-id {} --metastore-uri {} --grace-period 2s",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "Index `my-index` successfully garbage collected",
    ));
    assert_eq!(split_path.exists(), false);

    Ok(())
}

#[tokio::test]
#[cfg_attr(not(feature = "ci-test"), ignore)]
async fn test_cmd_dry_run_delete_on_s3_localstack() -> Result<()> {
    let index_id = "s3_index_0";
    let test_env = create_test_env(TestStorageType::S3)?;
    let index_uri = test_env.index_uri(index_id);
    make_command(
        format!(
            "new --index-uri {} --metastore-uri {} --index-config-path {}",
            index_uri,
            test_env.metastore_uri,
            test_env.resource_files["config"].display()
        )
        .as_str(),
    )
    .assert()
    .success();

    index_data(
        index_id,
        test_env.resource_files["logs"].as_path(),
        &test_env.metastore_uri,
    );

    make_command(
        format!(
            "gc --index-id {} --metastore-uri {}",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "No dangling files to garbage collect",
    ));

    make_command(
        format!(
            "delete --index-id {} --metastore-uri {} --dry-run",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::str::contains(
        "The following files will be removed",
    ))
    .stdout(predicate::str::contains(".split"));

    make_command(
        format!(
            "delete --index-id {} --metastore-uri {}",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success();

    Ok(())
}

/// testing the api via cli commands
#[tokio::test]
#[serial]
async fn test_all_local_index() -> Result<()> {
    // Implicit index_id defined in test env struct.
    // TODO: change that after the metastore uri refactoring.
    let index_id = "data";
    let test_env = create_test_env(TestStorageType::LocalFileSystem)?;
    make_command(
        format!(
            "new --index-uri {} --metastore-uri {} --index-config-path {}",
            test_env.index_uri(index_id),
            test_env.metastore_uri,
            test_env.resource_files["config"].display()
        )
        .as_str(),
    )
    .assert()
    .success();

    let metadata_file_exists = test_env
        .storage
        .exists(&Path::new(index_id).join("quickwit.json"))
        .await?;
    assert_eq!(metadata_file_exists, true);

    index_data(
        index_id,
        test_env.resource_files["logs"].as_path(),
        &test_env.metastore_uri,
    );

    // serve & api-search
    let mut server_process = spawn_command(
        format!(
            "serve --metastore-uri {} --host 127.0.0.1 --port 8182",
            test_env.metastore_uri,
        )
        .as_str(),
    )
    .unwrap();
    sleep(Duration::from_secs(2)).await;
    let mut data = vec![0; 512];
    server_process
        .stdout
        .as_mut()
        .expect("Failed to get server process output")
        .read_exact(&mut data)
        .expect("Cannot read output");
    let process_output_str = String::from_utf8(data).unwrap();
    let query_response = reqwest::get(format!(
        "http://127.0.0.1:8182/api/v1/{}/search?query=level:info",
        index_id
    ))
    .await?
    .text()
    .await?;

    assert!(process_output_str.contains("http://127.0.0.1:8182"));
    let result: Value =
        serde_json::from_str(&query_response).expect("Couldn't deserialize response.");
    assert_eq!(result["numHits"], Value::Number(Number::from(2i64)));

    let search_stream_response = reqwest::get(format!(
        "http://127.0.0.1:8182/api/v1/{}/search/stream?query=level:info&outputFormat=csv&fastField=ts",
        index_id
    ))
    .await?
    .text()
    .await?;
    assert_eq!(search_stream_response, "2\n13\n");

    server_process.kill().unwrap();

    make_command(
        format!(
            "delete --index-id {} --metastore-uri {}",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success();
    let metadata_file_exists = test_env
        .storage
        .exists(&Path::new(index_id).join("quickwit.json"))
        .await?;
    assert_eq!(metadata_file_exists, false);

    Ok(())
}

/// testing the api via cli commands
#[tokio::test]
#[serial]
#[cfg_attr(not(feature = "ci-test"), ignore)]
async fn test_all_with_s3_localstack_cli() -> Result<()> {
    let index_id = "s3_index_1";
    let test_env = create_test_env(TestStorageType::S3)?;
    let index_uri = test_env.index_uri(index_id);

    make_command(
        format!(
            "new --index-uri {} --metastore-uri {} --index-config-path {}",
            index_uri,
            test_env.metastore_uri,
            test_env.resource_files["config"].display()
        )
        .as_str(),
    )
    .assert()
    .success();

    let index_metadata = test_env.metastore().index_metadata(index_id).await;
    assert_eq!(index_metadata.is_ok(), true);

    index_data(
        index_id,
        test_env.resource_files["logs"].as_path(),
        &test_env.metastore_uri,
    );

    // cli search
    make_command(
        format!(
            "search --index-id {} --metastore-uri {} --query level:info",
            index_id, test_env.metastore_uri,
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::function(|output: &[u8]| {
        let result: Value = serde_json::from_slice(output).unwrap();
        result["numHits"] == Value::Number(Number::from(2i64))
    }));

    // serve & api-search
    let mut server_process = spawn_command(
        format!(
            "serve --metastore-uri {} --host 127.0.0.1 --port 8182",
            test_env.metastore_uri,
        )
        .as_str(),
    )
    .unwrap();
    sleep(Duration::from_secs(2)).await;
    let mut data = vec![0; 512];
    server_process
        .stdout
        .as_mut()
        .expect("Failed to get server process output")
        .read_exact(&mut data)
        .expect("Cannot read output");
    let process_output_str = String::from_utf8(data).unwrap();
    let query_response = reqwest::get(format!(
        "http://127.0.0.1:8182/api/v1/{}/search?query=level:info",
        index_id,
    ))
    .await?
    .text()
    .await?;
    server_process.kill().unwrap();

    assert!(process_output_str.contains("http://127.0.0.1:8182"));
    let result: Value =
        serde_json::from_str(&query_response).expect("Couldn't deserialize response.");
    assert_eq!(result["numHits"], Value::Number(Number::from(2i64)));

    make_command(
        format!(
            "delete --index-id {} --metastore-uri {}",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success();
    assert_eq!(test_env.storage.exists(Path::new(index_id)).await?, false);

    Ok(())
}

/// testing the api via structs of the lib (if available)
#[tokio::test]
#[serial]
#[cfg_attr(not(feature = "ci-test"), ignore)]
async fn test_all_with_s3_localstack_internal_api() -> Result<()> {
    let index_id = "s3_index_2";
    let test_env = create_test_env(TestStorageType::S3)?;
    let index_uri = test_env.index_uri(index_id);
    let args = CreateIndexArgs::new(
        test_env.metastore_uri.clone(),
        index_uri.clone(),
        test_env.resource_files["config"].to_path_buf(),
        false,
    )?;
    create_index_cli(args).await?;
    let index_metadata = test_env.metastore().index_metadata(index_id).await;
    assert_eq!(index_metadata.is_ok(), true);

    index_data(
        index_id,
        test_env.resource_files["logs"].as_path(),
        &test_env.metastore_uri,
    );

    // cli search
    make_command(
        format!(
            "search --index-id {} --metastore-uri {} --query level:info",
            index_id, test_env.metastore_uri,
        )
        .as_str(),
    )
    .assert()
    .success()
    .stdout(predicate::function(|output: &[u8]| {
        let result: Value = serde_json::from_slice(output).unwrap();
        result["numHits"] == Value::Number(Number::from(2i64))
    }));

    // serve & api-search
    let mut server_process = spawn_command(
        format!(
            "serve --metastore-uri {} --host 127.0.0.1 --port 8182",
            test_env.metastore_uri,
        )
        .as_str(),
    )
    .unwrap();
    sleep(Duration::from_secs(2)).await;
    let mut data = vec![0; 512];
    server_process
        .stdout
        .as_mut()
        .expect("Failed to get server process output")
        .read_exact(&mut data)
        .expect("Cannot read output");
    let process_output_str = String::from_utf8(data).unwrap();
    let query_response = reqwest::get(format!(
        "http://127.0.0.1:8182/api/v1/{}/search?query=level:info",
        index_id,
    ))
    .await?
    .text()
    .await?;
    server_process.kill().unwrap();

    assert!(process_output_str.contains("http://127.0.0.1:8182"));
    let result: Value =
        serde_json::from_str(&query_response).expect("Couldn't deserialize response.");
    assert_eq!(result["numHits"], Value::Number(Number::from(2i64)));

    make_command(
        format!(
            "delete --index-id {} --metastore-uri {}",
            index_id, test_env.metastore_uri
        )
        .as_str(),
    )
    .assert()
    .success();
    assert_eq!(test_env.storage.exists(Path::new(index_id)).await?, false);

    Ok(())
}
