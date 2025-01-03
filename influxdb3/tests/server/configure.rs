use hyper::StatusCode;
use observability_deps::tracing::debug;
use pretty_assertions::assert_eq;
use serde_json::{json, Value};
use test_helpers::assert_contains;

use crate::TestServer;

#[tokio::test]
async fn api_v3_configure_last_cache_create() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();
    let url = format!(
        "{base}/api/v3/configure/last_cache",
        base = server.client_addr()
    );

    // Write some LP to the database to initialize the catalog:
    let db_name = "db";
    let tbl_name = "tbl";
    server
        .write_lp_to_db(
            db_name,
            format!("{tbl_name},t1=a,t2=b,t3=c f1=true,f2=\"hello\",f3=4i,f4=4u,f5=5 1000"),
            influxdb3_client::Precision::Second,
        )
        .await
        .expect("write to db");

    #[derive(Default)]
    struct TestCase {
        // These attributes all map to parameters of the request body:
        description: &'static str,
        db: Option<&'static str>,
        table: Option<&'static str>,
        cache_name: Option<&'static str>,
        count: Option<usize>,
        ttl: Option<usize>,
        key_cols: Option<&'static [&'static str]>,
        val_cols: Option<&'static [&'static str]>,
        // This is the status code expected in the response:
        expected: StatusCode,
    }

    let test_cases = [
        TestCase {
            description: "no parameters specified",
            expected: StatusCode::BAD_REQUEST,
            ..Default::default()
        },
        TestCase {
            description: "missing database name",
            table: Some(tbl_name),
            expected: StatusCode::BAD_REQUEST,
            ..Default::default()
        },
        TestCase {
            description: "missing table name",
            db: Some(db_name),
            expected: StatusCode::BAD_REQUEST,
            ..Default::default()
        },
        TestCase {
            description: "Good, will use defaults for everything omitted, and get back a 201",
            db: Some(db_name),
            table: Some(tbl_name),
            expected: StatusCode::CREATED,
            ..Default::default()
        },
        TestCase {
            description: "Same as before, will be successful, but with 204",
            db: Some(db_name),
            table: Some(tbl_name),
            expected: StatusCode::NO_CONTENT,
            ..Default::default()
        },
        // NOTE: this will only differ from the previous cache in name, should this actually
        // be an error?
        TestCase {
            description: "Use a specific cache name, will succeed and create new cache",
            db: Some(db_name),
            table: Some(tbl_name),
            cache_name: Some("my_cache"),
            expected: StatusCode::CREATED,
            ..Default::default()
        },
        TestCase {
            description: "Same as previous, but will get 204 because it does nothing",
            db: Some(db_name),
            table: Some(tbl_name),
            cache_name: Some("my_cache"),
            expected: StatusCode::NO_CONTENT,
            ..Default::default()
        },
        TestCase {
            description: "Same as previous, but this time try to use different parameters, this \
            will result in a bad request",
            db: Some(db_name),
            table: Some(tbl_name),
            cache_name: Some("my_cache"),
            // The default TTL that would have been used is 4 * 60 * 60 seconds (4 hours)
            ttl: Some(666),
            expected: StatusCode::BAD_REQUEST,
            ..Default::default()
        },
        TestCase {
            description:
                "Will create new cache, because key columns are unique, and so will be the name",
            db: Some(db_name),
            table: Some(tbl_name),
            key_cols: Some(&["t1", "t2"]),
            expected: StatusCode::CREATED,
            ..Default::default()
        },
        TestCase {
            description: "Same as previous, but will get 204 because nothing happens",
            db: Some(db_name),
            table: Some(tbl_name),
            key_cols: Some(&["t1", "t2"]),
            expected: StatusCode::NO_CONTENT,
            ..Default::default()
        },
        TestCase {
            description: "Use an invalid key column (by name) is a bad request",
            db: Some(db_name),
            table: Some(tbl_name),
            key_cols: Some(&["not_a_key_column"]),
            expected: StatusCode::BAD_REQUEST,
            ..Default::default()
        },
        TestCase {
            description: "Use an invalid key column (by type) is a bad request",
            db: Some(db_name),
            table: Some(tbl_name),
            // f5 is a float, which is not supported as a key column:
            key_cols: Some(&["f5"]),
            expected: StatusCode::BAD_REQUEST,
            ..Default::default()
        },
        TestCase {
            description: "Use an invalid value column is a bad request",
            db: Some(db_name),
            table: Some(tbl_name),
            val_cols: Some(&["not_a_value_column"]),
            expected: StatusCode::BAD_REQUEST,
            ..Default::default()
        },
        TestCase {
            description: "Use an invalid cache size is a bad request",
            db: Some(db_name),
            table: Some(tbl_name),
            count: Some(11),
            expected: StatusCode::BAD_REQUEST,
            ..Default::default()
        },
    ];

    for (i, t) in test_cases.into_iter().enumerate() {
        let body = serde_json::json!({
            "db": t.db,
            "table": t.table,
            "name": t.cache_name,
            "key_columns": t.key_cols,
            "value_columns": t.val_cols,
            "count": t.count,
            "ttl": t.ttl,
        });
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .expect("send /api/v3/configure/last_cache request");
        let status = resp.status();
        assert_eq!(
            t.expected,
            status,
            "test case ({i}) failed, {description}",
            description = t.description
        );
    }
}

#[tokio::test]
async fn api_v3_configure_last_cache_delete() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();
    let url = format!(
        "{base}/api/v3/configure/last_cache",
        base = server.client_addr()
    );

    // Write some LP to the database to initialize the catalog:
    let db_name = "db";
    let tbl_name = "tbl";
    let cache_name = "test_cache";
    server
        .write_lp_to_db(
            db_name,
            format!("{tbl_name},t1=a,t2=b,t3=c f1=true,f2=\"hello\",f3=4i,f4=4u,f5=5 1000"),
            influxdb3_client::Precision::Second,
        )
        .await
        .expect("write to db");

    struct TestCase {
        request: Request,
        // This is the status code expected in the response:
        expected: StatusCode,
    }

    enum Request {
        Create(serde_json::Value),
        Delete(DeleteRequest),
    }

    #[derive(Default)]
    struct DeleteRequest {
        db: Option<&'static str>,
        table: Option<&'static str>,
        name: Option<&'static str>,
    }

    use Request::*;
    let mut test_cases = [
        // Create a cache:
        TestCase {
            request: Create(serde_json::json!({
                "db": db_name,
                "table": tbl_name,
                "name": cache_name,
            })),
            expected: StatusCode::CREATED,
        },
        // Missing all params:
        TestCase {
            request: Delete(DeleteRequest {
                ..Default::default()
            }),
            expected: StatusCode::BAD_REQUEST,
        },
        // Partial params:
        TestCase {
            request: Delete(DeleteRequest {
                db: Some(db_name),
                ..Default::default()
            }),
            expected: StatusCode::BAD_REQUEST,
        },
        // Partial params:
        TestCase {
            request: Delete(DeleteRequest {
                table: Some(tbl_name),
                ..Default::default()
            }),
            expected: StatusCode::BAD_REQUEST,
        },
        // Partial params:
        TestCase {
            request: Delete(DeleteRequest {
                name: Some(cache_name),
                ..Default::default()
            }),
            expected: StatusCode::BAD_REQUEST,
        },
        // Partial params:
        TestCase {
            request: Delete(DeleteRequest {
                db: Some(db_name),
                table: Some(tbl_name),
                ..Default::default()
            }),
            expected: StatusCode::BAD_REQUEST,
        },
        // All params, good:
        TestCase {
            request: Delete(DeleteRequest {
                db: Some(db_name),
                table: Some(tbl_name),
                name: Some(cache_name),
            }),
            expected: StatusCode::OK,
        },
        // Same as previous, with correct parameters provided, but gest 404, as its already deleted:
        TestCase {
            request: Delete(DeleteRequest {
                db: Some(db_name),
                table: Some(tbl_name),
                name: Some(cache_name),
            }),
            expected: StatusCode::NOT_FOUND,
        },
    ];

    // Do one pass using the JSON body to delete:
    for (i, t) in test_cases.iter().enumerate() {
        match &t.request {
            Create(body) => assert!(
                client
                    .post(&url)
                    .json(&body)
                    .send()
                    .await
                    .expect("create request succeeds")
                    .status()
                    .is_success(),
                "Creation test case ({i}) failed"
            ),
            Delete(req) => {
                let body = serde_json::json!({
                    "db": req.db,
                    "table": req.table,
                    "name": req.name,
                });
                let resp = client
                    .delete(&url)
                    .json(&body)
                    .send()
                    .await
                    .expect("send /api/v3/configure/last_cache request");
                let status = resp.status();
                assert_eq!(
                    t.expected, status,
                    "Deletion test case ({i}) using JSON body failed"
                );
            }
        }
    }

    // Do another pass using the URI query string to delete:
    // Note: this particular test exhibits different status code, because the empty query string
    // as a result of there being no parameters provided makes the request handler attempt to
    // parse the body as JSON - which gives a 415 error, because there is no body or content type
    test_cases[1].expected = StatusCode::UNSUPPORTED_MEDIA_TYPE;
    for (i, t) in test_cases.iter().enumerate() {
        match &t.request {
            Create(body) => assert!(
                client
                    .post(&url)
                    .json(&body)
                    .send()
                    .await
                    .expect("create request succeeds")
                    .status()
                    .is_success(),
                "Creation test case ({i}) failed"
            ),
            Delete(req) => {
                let mut params = vec![];
                if let Some(db) = req.db {
                    params.push(("db", db));
                }
                if let Some(table) = req.table {
                    params.push(("table", table));
                }
                if let Some(name) = req.name {
                    params.push(("name", name));
                }
                let resp = client
                    .delete(&url)
                    .query(&params)
                    .send()
                    .await
                    .expect("send /api/v3/configure/last_cache request");
                let status = resp.status();
                assert_eq!(
                    t.expected, status,
                    "Deletion test case ({i}) using URI query string failed"
                );
            }
        }
    }
}

#[test_log::test(tokio::test)]
async fn api_v3_configure_db_delete() {
    let db_name = "foo";
    let tbl_name = "tbl";
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();
    let url = format!(
        "{base}/api/v3/configure/database?db={db_name}",
        base = server.client_addr()
    );

    server
        .write_lp_to_db(
            db_name,
            format!("{tbl_name},t1=a,t2=b,t3=c f1=true,f2=\"hello\",f3=4i,f4=4u,f5=5 1000"),
            influxdb3_client::Precision::Second,
        )
        .await
        .expect("write to db");

    // check foo db is present
    let result = server
        .api_v3_query_influxql(&[("q", "SHOW DATABASES"), ("format", "json")])
        .await
        .json::<Value>()
        .await
        .unwrap();
    debug!(result = ?result, ">> RESULT");
    assert_eq!(json!([{ "iox::database": "foo" } ]), result);

    let resp = client
        .delete(&url)
        .send()
        .await
        .expect("delete database call succeed");
    assert_eq!(200, resp.status());

    // check foo db is now foo-YYYYMMDD..
    let result = server
        .api_v3_query_influxql(&[("q", "SHOW DATABASES"), ("format", "json")])
        .await
        .json::<Value>()
        .await
        .unwrap();
    debug!(result = ?result, ">> RESULT");
    let array_result = result.as_array().unwrap();
    assert_eq!(1, array_result.len());
    let first_db = array_result.first().unwrap();
    assert_contains!(
        first_db
            .as_object()
            .unwrap()
            .get("iox::database")
            .unwrap()
            .as_str()
            .unwrap(),
        "foo-"
    );

    server
        .write_lp_to_db(
            db_name,
            format!("{tbl_name},t1=a,t2=b,t3=c f1=true,f2=\"hello\",f3=4i,f4=4u,f5=5 1000"),
            influxdb3_client::Precision::Second,
        )
        .await
        .expect("write to db");

    let result = server
        .api_v3_query_influxql(&[("q", "SHOW DATABASES"), ("format", "json")])
        .await
        .json::<Value>()
        .await
        .unwrap();
    debug!(result = ?result, ">> RESULT");
    let array_result = result.as_array().unwrap();
    // check there are 2 dbs now, foo and foo-*
    assert_eq!(2, array_result.len());
    let first_db = array_result.first().unwrap();
    let second_db = array_result.get(1).unwrap();
    assert_eq!(
        "foo",
        first_db
            .as_object()
            .unwrap()
            .get("iox::database")
            .unwrap()
            .as_str()
            .unwrap(),
    );
    assert_contains!(
        second_db
            .as_object()
            .unwrap()
            .get("iox::database")
            .unwrap()
            .as_str()
            .unwrap(),
        "foo-"
    );
}

#[tokio::test]
async fn api_v3_configure_db_delete_no_db() {
    let db_name = "db";
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();
    let url = format!(
        "{base}/api/v3/configure/database?db={db_name}",
        base = server.client_addr()
    );

    let resp = client
        .delete(&url)
        .send()
        .await
        .expect("delete database call succeed");
    assert_eq!(StatusCode::NOT_FOUND, resp.status());
}

#[tokio::test]
async fn api_v3_configure_db_delete_missing_query_param() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();
    let url = format!(
        "{base}/api/v3/configure/database",
        base = server.client_addr()
    );

    let resp = client
        .delete(&url)
        .send()
        .await
        .expect("delete database call succeed");
    assert_eq!(StatusCode::BAD_REQUEST, resp.status());
}

#[test_log::test(tokio::test)]
async fn api_v3_configure_table_delete() {
    let db_name = "foo";
    let tbl_name = "tbl";
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();
    let url = format!(
        "{base}/api/v3/configure/table?db={db_name}&table={tbl_name}",
        base = server.client_addr()
    );

    server
        .write_lp_to_db(
            db_name,
            format!("{tbl_name},t1=a,t2=b,t3=c f1=true,f2=\"hello\",f3=4i,f4=4u,f5=5 1000"),
            influxdb3_client::Precision::Second,
        )
        .await
        .expect("write to db");

    let resp = client
        .delete(&url)
        .send()
        .await
        .expect("delete table call succeed");
    assert_eq!(200, resp.status());

    // check foo db has table with name in tbl-YYYYMMDD.. format
    let result = server
        .api_v3_query_influxql(&[("q", "SHOW MEASUREMENTS on foo"), ("format", "json")])
        .await
        .json::<Value>()
        .await
        .unwrap();
    debug!(result = ?result, ">> RESULT");
    let array_result = result.as_array().unwrap();
    assert_eq!(1, array_result.len());
    let first_db = array_result.first().unwrap();
    assert_contains!(
        first_db
            .as_object()
            .unwrap()
            .get("name")
            .unwrap()
            .as_str()
            .unwrap(),
        "tbl-"
    );

    server
        .write_lp_to_db(
            db_name,
            format!("{tbl_name},t1=a,t2=b,t3=c f1=true,f2=\"hello\",f3=4i,f4=4u,f5=5 1000"),
            influxdb3_client::Precision::Second,
        )
        .await
        .expect("write to db");

    let result = server
        .api_v3_query_influxql(&[("q", "SHOW MEASUREMENTS on foo"), ("format", "json")])
        .await
        .json::<Value>()
        .await
        .unwrap();
    debug!(result = ?result, ">> RESULT");
    let array_result = result.as_array().unwrap();
    // check there are 2 tables now, tbl and tbl-*
    assert_eq!(2, array_result.len());
    let first_db = array_result.first().unwrap();
    let second_db = array_result.get(1).unwrap();
    assert_eq!(
        "tbl",
        first_db
            .as_object()
            .unwrap()
            .get("name")
            .unwrap()
            .as_str()
            .unwrap(),
    );
    assert_contains!(
        second_db
            .as_object()
            .unwrap()
            .get("name")
            .unwrap()
            .as_str()
            .unwrap(),
        "tbl-"
    );
}

#[tokio::test]
async fn api_v3_configure_table_delete_no_db() {
    let db_name = "db";
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();
    let url = format!(
        "{base}/api/v3/configure/table?db={db_name}&table=foo",
        base = server.client_addr()
    );

    let resp = client
        .delete(&url)
        .send()
        .await
        .expect("delete database call succeed");
    assert_eq!(StatusCode::NOT_FOUND, resp.status());
}

#[tokio::test]
async fn api_v3_configure_table_delete_missing_query_param() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();
    let url = format!("{base}/api/v3/configure/table", base = server.client_addr());

    let resp = client
        .delete(&url)
        .send()
        .await
        .expect("delete table call succeed");
    assert_eq!(StatusCode::BAD_REQUEST, resp.status());
}

#[tokio::test]
async fn try_deleting_table_after_db_is_deleted() {
    let db_name = "db";
    let tbl_name = "tbl";
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();
    let delete_db_url = format!(
        "{base}/api/v3/configure/database?db={db_name}",
        base = server.client_addr()
    );
    let delete_table_url = format!(
        "{base}/api/v3/configure/table?db={db_name}&table={tbl_name}",
        base = server.client_addr()
    );
    server
        .write_lp_to_db(
            db_name,
            format!("{tbl_name},t1=a,t2=b,t3=c f1=true,f2=\"hello\",f3=4i,f4=4u,f5=5 1000"),
            influxdb3_client::Precision::Second,
        )
        .await
        .expect("write to db");

    // db call should succeed
    let resp = client
        .delete(&delete_db_url)
        .send()
        .await
        .expect("delete database call succeed");

    assert_eq!(StatusCode::OK, resp.status());

    // but table delete call should fail with NOT_FOUND
    let resp = client
        .delete(&delete_table_url)
        .send()
        .await
        .expect("delete table call succeed");
    assert_eq!(StatusCode::NOT_FOUND, resp.status());
}
