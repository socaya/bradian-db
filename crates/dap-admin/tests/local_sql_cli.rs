//! Exercise the actual operator executable across process boundaries.
use std::process::Command;
#[cfg(unix)]
use std::process::{Output, Stdio};
#[cfg(unix)]
use serde_json::Value;

// Process spawning can transiently inherit another fixture's advisory lock
// before exec. Serialize fixtures and child processes within this test binary.
static PROCESS_TESTS: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn binary() -> &'static str { env!("CARGO_BIN_EXE_bradian-db") }

#[test]
fn canonical_and_legacy_operator_executables_remain_available() {
    let _serial = PROCESS_TESTS.lock().unwrap();
    for path in [binary(), env!("CARGO_BIN_EXE_dap-admin")] {
        let output = Command::new(path).arg("version").output().unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8(output.stdout).unwrap().starts_with("bradian-db "));
        let output = Command::new(path).args(["sql","--help"]).output().unwrap();
        assert!(output.status.success());
        let help=String::from_utf8(output.stdout).unwrap(); assert!(help.contains("--history")); assert!(help.contains("--max-output-bytes"));
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::path::Path;
    use std::os::unix::fs::PermissionsExt;
    use std::io::Write;
    use bradian_relational::sql::SqlEngine;

    fn run(root: &Path, sql: &str, flags: &[&str]) -> Output {
        Command::new(binary()).args(["sql","--root"]).arg(root).args(flags).args(["--execute",sql]).output().unwrap()
    }
    fn ok(output: Output) -> Vec<Value> {
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap().lines().map(|line|serde_json::from_str(line).unwrap()).collect()
    }
    fn init(root: &Path) {
        ok(run(root, "CREATE DATABASE test; CREATE SCHEMA test.public; CREATE TABLE test.public.items (id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id)); INSERT INTO test.public.items(id,amount) VALUES(1,100)", &[]));
    }
    fn amount(root: &Path) -> String { ok(run(root,"SELECT amount FROM test.public.items WHERE id=1",&[]))[0]["rows"][0][0]["value"].as_str().unwrap().into() }

    #[test]
    fn output_budgets_apply_to_autocommit_and_scoped_modes_without_partial_lines() {
        let _serial=PROCESS_TESTS.lock().unwrap(); let dir=tempfile::tempdir().unwrap(); let root=dir.path().join("db"); init(&root);
        let query="SELECT amount FROM test.public.items WHERE id=1"; let baseline=run(&root,query,&[]); assert!(baseline.status.success()); let n=baseline.stdout.len();
        let exact=n.to_string(); assert_eq!(run(&root,query,&["--max-output-bytes",&exact]).stdout,baseline.stdout);
        let low=(n-1).to_string(); let failed=run(&root,query,&["--max-output-bytes",&low]); assert!(!failed.status.success()); assert!(failed.stdout.is_empty());
        let failed=run(&root,"BEGIN; UPDATE test.public.items SET amount=999 WHERE id=1; SELECT amount FROM test.public.items WHERE id=1; COMMIT",&["--table","test.public.items","--max-output-bytes",&low]);
        assert!(!failed.status.success()); assert!(String::from_utf8(failed.stderr).unwrap().contains("output byte limit exceeded"));
        let prior=String::from_utf8(failed.stdout).unwrap(); assert_eq!(prior.lines().count(),2); for line in prior.lines() { serde_json::from_str::<Value>(line).unwrap(); }
        assert_eq!(amount(&root),"100");
        let failed=run(&root,"UPDATE test.public.items SET amount=777 WHERE id=1",&["--max-output-bytes","0"]);
        assert!(!failed.status.success()); assert!(failed.stdout.is_empty()); assert_eq!(amount(&root),"777"); // autocommit preceded output refusal
    }

    #[test]
    fn native_create_insert_select_and_transaction_commit_survive_process_restart() {
        let _serial = PROCESS_TESTS.lock().unwrap();
        let dir = tempfile::tempdir().unwrap(); let root = dir.path().join("db");
        init(&root);
        assert_eq!(amount(&root), "100");
        let rows = ok(run(&root,"BEGIN; UPDATE test.public.items SET amount=150 WHERE id=1; COMMIT; BEGIN; SELECT amount FROM test.public.items WHERE id=1; ROLLBACK", &["--table","test.public.items"]));
        assert_eq!(rows.len(),6);
        assert_eq!(rows[4]["rows"][0][0]["value"], "150");
        assert_eq!(amount(&root), "150");
        assert_eq!(std::fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700);
    }

    #[test]
    fn stdin_script_keeps_quoted_semicolons_and_escaped_unicode_text() {
        let _serial = PROCESS_TESTS.lock().unwrap();
        let dir = tempfile::tempdir().unwrap(); let root = dir.path().join("db");
        let mut child = Command::new(binary()).args(["sql","--root"]).arg(&root).args(["--file","-"])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
        child.stdin.take().unwrap().write_all("CREATE DATABASE test; CREATE SCHEMA test.public; CREATE TABLE test.public.texts(id BIGINT NOT NULL, body TEXT, PRIMARY KEY(id)); INSERT INTO test.public.texts(id,body) VALUES(1,'é;it''s -- text'); SELECT body FROM test.public.texts WHERE id=1".as_bytes()).unwrap();
        let rows = ok(child.wait_with_output().unwrap());
        assert_eq!(rows[4]["rows"][0][0]["value"], "é;it's -- text");
    }

    #[test]
    fn unfinished_or_failed_transaction_scripts_rollback_all_staged_rows() {
        let _serial = PROCESS_TESTS.lock().unwrap();
        let dir = tempfile::tempdir().unwrap(); let root = dir.path().join("db"); init(&root);
        for script in ["BEGIN; UPDATE test.public.items SET amount=999 WHERE id=1",
            "BEGIN; UPDATE test.public.items SET amount=999 WHERE id=1; INSERT INTO test.public.items(id,amount) VALUES(1,1); COMMIT",
            "BEGIN; UPDATE test.public.items SET amount=999 WHERE id=1; SELECT missing FROM test.public.items; COMMIT"] {
            let output = run(&root,script,&["--table","test.public.items"]);
            assert!(!output.status.success());
            assert_eq!(amount(&root), "100");
        }
        ok(run(&root,"BEGIN; UPDATE test.public.items SET amount=999 WHERE id=1; ROLLBACK", &["--table","test.public.items"]));
        assert_eq!(amount(&root), "100");
    }

    #[test]
    fn first_execution_error_stops_script_without_echoing_sql_or_undoing_prior_commits() {
        let _serial = PROCESS_TESTS.lock().unwrap();
        let dir = tempfile::tempdir().unwrap(); let root = dir.path().join("db"); init(&root);
        let output = run(&root,"UPDATE test.public.items SET amount=101 WHERE id=1; INSERT INTO test.public.items(id,amount) VALUES(2,'private-marker'); DELETE FROM test.public.items WHERE id=1",&[]);
        assert!(!output.status.success());
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(error.contains("statement 2 failed"));
        assert!(!error.contains("private-marker"));
        assert!(!error.contains("INSERT INTO"));
        assert_eq!(amount(&root), "101");
    }

    #[test]
    fn malformed_utf8_comments_and_size_fail_before_creating_database() {
        let _serial = PROCESS_TESTS.lock().unwrap();
        let dir = tempfile::tempdir().unwrap(); let root = dir.path().join("db");
        for script in ["CREATE DATABASE test; SELECT 'open", "CREATE DATABASE test; -- comment", " ; "] {
            assert!(!run(&root,script,&[]).status.success());
            assert!(!root.exists());
        }
        let path = dir.path().join("script.sql");
        for bytes in [vec![255], vec![b'x';1024*1024+1]] {
            std::fs::write(&path,bytes).unwrap();
            let output = Command::new(binary()).args(["sql","--root"]).arg(&root).arg("--file").arg(&path).output().unwrap();
            assert!(!output.status.success()); assert!(!root.exists());
        }
    }

    #[test]
    fn public_symlink_and_non_directory_roots_are_refused_without_changing_permissions() {
        let _serial = PROCESS_TESTS.lock().unwrap();
        let dir = tempfile::tempdir().unwrap(); let root = dir.path().join("db");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root,std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!run(&root,"CREATE DATABASE test",&[]).status.success());
        assert_eq!(std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,0o755);
        assert!(!root.join("catalog").exists());
        std::fs::set_permissions(&root,std::fs::Permissions::from_mode(0o700)).unwrap();
        let link = dir.path().join("alias"); std::os::unix::fs::symlink(&root,&link).unwrap();
        assert!(!run(&link,"CREATE DATABASE test",&[]).status.success());
        let file = dir.path().join("not-directory"); std::fs::write(&file,b"untouched").unwrap();
        assert!(!run(&file,"CREATE DATABASE test",&[]).status.success());
        assert_eq!(std::fs::read(file).unwrap(), b"untouched");
    }

    #[test]
    fn conflicting_input_modes_are_rejected_before_opening_database() {
        let _serial = PROCESS_TESTS.lock().unwrap();
        let dir = tempfile::tempdir().unwrap(); let root = dir.path().join("db");
        let both = Command::new(binary()).args(["sql","--root"]).arg(&root).args(["--file","missing","--execute","SELECT x"]).output().unwrap();
        assert!(!both.status.success());
        assert!(!run(&root,"BEGIN",&["--table","test.public.items","--history"]).status.success());
        assert!(!run(&root,"BEGIN",&["--history"]).status.success());
        for flag in ["--max-history-keys","--max-history-versions","--max-history-bytes","--max-staged-keys","--max-staged-bytes","--max-active-transactions"] {
            assert!(!run(&root,"BEGIN",&[flag,"1"]).status.success());
        }
        assert!(!root.exists());
    }

    #[test]
    fn scan_flags_are_enforced_in_autocommit_and_single_table_modes() {
        let _serial = PROCESS_TESTS.lock().unwrap();
        let dir = tempfile::tempdir().unwrap(); let root = dir.path().join("db"); init(&root);
        for flag in ["--max-scan-rows","--max-scan-bytes","--max-scan-work"] {
            let failed = run(&root,"SELECT amount FROM test.public.items",&[flag,"0"]);
            assert!(!failed.status.success());
            assert!(String::from_utf8(failed.stderr).unwrap().contains("budget exceeded"));
            let failed = run(&root,"BEGIN; UPDATE test.public.items SET amount=999 WHERE id=1; COMMIT",&["--table","test.public.items",flag,"0"]);
            assert!(!failed.status.success());
            assert_eq!(amount(&root), "100");
        }
        ok(run(&root,"BEGIN; SELECT id FROM test.public.items; COMMIT", &["--table","test.public.items","--max-scan-rows","1","--max-scan-bytes","1000","--max-scan-work","2"]));
        assert_eq!(amount(&root), "100");
    }

    #[test]
    fn second_process_cannot_bypass_existing_catalog_lock() {
        let _serial = PROCESS_TESTS.lock().unwrap();
        let dir = tempfile::tempdir().unwrap(); let root = dir.path().join("db"); init(&root);
        let engine = SqlEngine::open(&root).unwrap();
        let output = run(&root,"UPDATE test.public.items SET amount=999 WHERE id=1",&[]);
        assert!(!output.status.success());
        drop(engine);
        assert_eq!(amount(&root), "100");
    }

    #[test]
    fn historical_mode_uses_existing_durable_engine_without_implicit_migration() {
        use bradian_relational::{domain_sql::DomainSqlEngine, history_migration::migrate_shared_to_history, history_sql::HistorySqlEngine};
        let _serial = PROCESS_TESTS.lock().unwrap();
        let dir = tempfile::tempdir().unwrap(); let source = dir.path().join("source"); let root = dir.path().join("history");
        {
            let ddl = SqlEngine::open(&source).unwrap();
            for sql in ["CREATE DATABASE test","CREATE SCHEMA test.public","CREATE TABLE test.public.items(id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id))"] { ddl.execute(sql).unwrap(); }
        }
        {
            let db = DomainSqlEngine::open(&source).unwrap(); db.register_catalog_table("test.public.items").unwrap();
            db.with_session(|s| { s.execute("BEGIN")?; s.execute("INSERT INTO test.public.items(id,amount) VALUES(1,100)")?; s.execute("COMMIT")?; Ok(()) }).unwrap();
        }
        migrate_shared_to_history(&source,&root).unwrap();
        std::fs::set_permissions(&root,std::fs::Permissions::from_mode(0o700)).unwrap();
        {
            let mut db = HistorySqlEngine::open(&root).unwrap(); db.enable_epoch_row_writes().unwrap();
            db.execute_ddl("ALTER TABLE test.public.items ADD price NUMERIC(8,2) NOT NULL DEFAULT 1.25").unwrap();
        }
        let rows = ok(run(&root,"BEGIN WITH READ VALIDATION; SAVEPOINT before_update; UPDATE test.public.items SET amount=amount+5,price=price*2.00 WHERE id=1; SELECT amount,price FROM test.public.items ORDER BY amount DESC LIMIT 1; COMMIT", &["--history"]));
        assert_eq!(rows[3]["rows"][0][0]["value"], "105"); assert_eq!(rows[3]["rows"][0][1]["unscaled"], "250");
        let failed = run(&root,"BEGIN; UPDATE test.public.items SET amount=999 WHERE id=1",&["--history"]);
        assert!(!failed.status.success());
        let rows = ok(run(&root,"BEGIN; SELECT amount FROM test.public.items WHERE id=1; COMMIT",&["--history"]));
        assert_eq!(rows[1]["rows"][0][0]["value"], "105");
        for flag in ["--max-scan-rows","--max-scan-bytes","--max-scan-work"] {
            let output = run(&root,"BEGIN; UPDATE test.public.items SET amount=999 WHERE id=1; SELECT id FROM test.public.items; COMMIT", &["--history",flag,"0"]);
            assert!(!output.status.success());
            assert!(String::from_utf8(output.stderr).unwrap().contains("budget exceeded"));
        }
        let rows = ok(run(&root,"BEGIN; SELECT amount FROM test.public.items; COMMIT",&["--history","--max-scan-rows","1","--max-scan-bytes","1000","--max-scan-work","2"]));
        assert_eq!(rows[1]["rows"][0][0]["value"], "105");
        assert!(!run(&root,"SELECT amount FROM test.public.items",&[]).status.success());
        for flag in ["--max-history-keys","--max-history-versions","--max-history-bytes"] {
            let output = run(&root,"BEGIN; SELECT id FROM test.public.items; COMMIT",&["--history",flag,"0"]);
            assert!(!output.status.success());
            assert!(String::from_utf8(output.stderr).unwrap().contains("retained history"));
        }
        for flag in ["--max-staged-keys","--max-staged-bytes"] {
            ok(run(&root,"BEGIN; SELECT id FROM test.public.items; COMMIT",&["--history",flag,"0"]));
            let output=run(&root,"BEGIN; UPDATE test.public.items SET amount=999 WHERE id=1; COMMIT",&["--history",flag,"0"]);
            assert!(!output.status.success());
            assert!(String::from_utf8(output.stderr).unwrap().contains("transaction staging"));
        }
        ok(run(&root,"BEGIN; UPDATE test.public.items SET amount=105 WHERE id=1; COMMIT",&["--history","--max-staged-keys","1","--max-staged-bytes","1000"]));
        let versions = { let db=HistorySqlEngine::open(&root).unwrap(); db.history_usage().versions+1 };
        let capacity=versions.to_string();
        ok(run(&root,"BEGIN; UPDATE test.public.items SET amount=106 WHERE id=1; COMMIT",&["--history","--max-history-versions",&capacity]));
        let failed=run(&root,"BEGIN; UPDATE test.public.items SET amount=999 WHERE id=1; COMMIT",&["--history","--max-history-versions",&capacity]);
        assert!(!failed.status.success());
        assert!(String::from_utf8(failed.stderr).unwrap().contains("retained history versions"));
        let rows=ok(run(&root,"BEGIN; SELECT amount FROM test.public.items WHERE id=1; COMMIT",&["--history","--max-history-versions",&capacity]));
        assert_eq!(rows[1]["rows"][0][0]["value"],"106");
        let refused=run(&root,"BEGIN; SELECT amount FROM test.public.items WHERE id=1; COMMIT",&["--history","--max-active-transactions","0"]);
        assert!(!refused.status.success()); assert!(refused.stdout.is_empty()); assert!(String::from_utf8(refused.stderr).unwrap().contains("active history transaction limit"));
        let rows=ok(run(&root,"BEGIN WITH READ VALIDATION; SELECT amount FROM test.public.items WHERE id=1; COMMIT",&["--history","--max-active-transactions","1"]));
        assert_eq!(rows[1]["rows"][0][0]["value"],"106");
        let baseline=run(&root,"BEGIN; SELECT amount FROM test.public.items WHERE id=1; COMMIT",&["--history"]);
        assert!(baseline.status.success()); let text=String::from_utf8(baseline.stdout).unwrap(); let n=text.lines().nth(1).unwrap().len()+1;
        let exact=n.to_string(); ok(run(&root,"BEGIN; SELECT amount FROM test.public.items WHERE id=1; COMMIT",&["--history","--max-output-bytes",&exact]));
        let low=(n-1).to_string(); let failed=run(&root,"BEGIN; UPDATE test.public.items SET amount=999 WHERE id=1; SELECT amount FROM test.public.items WHERE id=1; COMMIT",&["--history","--max-output-bytes",&low]);
        assert!(!failed.status.success()); assert!(String::from_utf8(failed.stderr).unwrap().contains("output byte limit exceeded"));
        let prior=String::from_utf8(failed.stdout).unwrap(); assert_eq!(prior.lines().count(),2); for line in prior.lines() { serde_json::from_str::<Value>(line).unwrap(); }
        let rows=ok(run(&root,"BEGIN; SELECT amount FROM test.public.items WHERE id=1; COMMIT",&["--history"])); assert_eq!(rows[1]["rows"][0][0]["value"],"106");
    }
}
