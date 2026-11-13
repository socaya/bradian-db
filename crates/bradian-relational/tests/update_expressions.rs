#[path = "support/epoch_fixture.rs"]
mod fixture;
use fixture::{migrated, modify, query};
use bradian_relational::history_sql::HistorySqlEngine;
use bradian_relational::history_backup::restore_history_database;
use bradian_relational::sql::{SqlEngine, SqlResultSet};
use bradian_relational::typed_table::Value;

fn extended(root: &std::path::Path) -> HistorySqlEngine {
    let mut db = HistorySqlEngine::open(root).unwrap();
    db.enable_epoch_row_writes().unwrap();
    for clause in ["other BIGINT NOT NULL DEFAULT 7", "small INTEGER NOT NULL DEFAULT 1",
        "price NUMERIC(8,2) NOT NULL DEFAULT 1.25", "wide NUMERIC(38,2) NOT NULL DEFAULT 1.00",
        "note TEXT DEFAULT 'new'", "copy TEXT", "flag BOOLEAN DEFAULT true"] {
        db.execute_ddl(&format!("ALTER TABLE bank.public.accounts ADD {clause}")).unwrap();
    }
    db
}

#[test]
fn precedence_signs_parentheses_and_assignments_use_the_original_row() {
    let (_dir, root) = migrated();
    let db = extended(&root);
    modify(&db, &["UPDATE bank.public.accounts SET balance=balance+other*2, other=balance-other WHERE id=1"]);
    assert_eq!(query(&db, "SELECT balance,other FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(114), Value::Int64(93)]]);
    modify(&db, &["UPDATE bank.public.accounts SET balance=(balance-14)/5 + - -1, other=-(other-3) WHERE id=1"]);
    assert_eq!(query(&db, "SELECT balance,other FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(21), Value::Int64(-90)]]);
    modify(&db, &["UPDATE bank.public.accounts SET balance=other, other=balance, copy=note WHERE id=1"]);
    assert_eq!(query(&db, "SELECT balance,other,copy FROM bank.public.accounts WHERE id=1"),
        vec![vec![Value::Int64(-90), Value::Int64(21), Value::String("new".into())]]);
    modify(&db, &["UPDATE bank.public.accounts SET balance=10-3-2, other=20/2/2, small=+3*-(1+1) WHERE id=1"]);
    assert_eq!(query(&db, "SELECT balance,other,small FROM bank.public.accounts WHERE id=1"),
        vec![vec![Value::Int64(5), Value::Int64(5), Value::Int32(-6)]]);
}

#[test]
fn signed_minima_negative_predicates_and_integer_division_remain_exact() {
    let (_dir, root) = migrated();
    let db = extended(&root);
    modify(&db, &["INSERT INTO bank.public.accounts(id,balance) VALUES(-1,-9223372036854775808)",
        "INSERT INTO bank.public.messages(id,body) VALUES(-1,'-- and /* are quoted text')",
        "UPDATE bank.public.accounts SET balance=-9223372036854775808, small=-2147483648 WHERE id=-1"]);
    assert_eq!(query(&db, "SELECT balance,small FROM bank.public.accounts WHERE id=-1"),
        vec![vec![Value::Int64(i64::MIN), Value::Int32(i32::MIN)]]);
    for expr in ["balance=balance/-1", "small=small/-1", "balance=-balance", "small=-small",
        "balance=9223372036854775807+1", "small=2147483647+1", "balance=-9223372036854775808-1"] {
        let before = db.last_lsn();
        db.with_session(|s| {
            s.execute("BEGIN")?;
            assert!(s.execute(&format!("UPDATE bank.public.accounts SET {expr} WHERE id=-1")).is_err(), "accepted {expr}");
            assert!(!s.in_transaction());
            Ok(())
        }).unwrap();
        assert_eq!(db.last_lsn(), before);
    }
    modify(&db, &["UPDATE bank.public.accounts SET balance=-7/2, small=7/-2 WHERE id=-1"]);
    assert_eq!(query(&db, "SELECT balance,small FROM bank.public.accounts WHERE id=-1"),
        vec![vec![Value::Int64(-3), Value::Int32(-3)]]);
}

#[test]
fn decimal_operations_are_exact_at_declared_scale_and_handle_maximum_precision() {
    let (_dir, root) = migrated();
    let db = extended(&root);
    for (expression, expected) in [("price*2.00",250), ("price/2.00",125),
        ("1.20/0.30",400), ("0.10*0.10",1), ("-1.50/0.50",-300), ("0.00/-1.00",0), ("1.20+2.30-0.50",300)] {
        modify(&db, &[&format!("UPDATE bank.public.accounts SET price={expression} WHERE id=1")]);
        assert_eq!(query(&db, "SELECT price FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Decimal(expected)]]);
    }
    let max = "999999999999999999999999999999999999.99";
    modify(&db, &[&format!("UPDATE bank.public.accounts SET wide={max} WHERE id=1"),
        "UPDATE bank.public.accounts SET wide=wide*1.00 WHERE id=1",
        "UPDATE bank.public.accounts SET wide=wide/1.00 WHERE id=1"]);
    assert_eq!(query(&db, "SELECT wide FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Decimal(10_i128.pow(38)-1)]]);
    for expr in ["price=1.00/3.00", "price=0.01*0.01", "price=999999.99+0.01", "price=1.00/0.00",
        "wide=wide+0.01", "wide=wide*2.00", "price=1.00/0.03"] {
        let before = db.last_lsn();
        db.with_session(|s| {
            s.execute("BEGIN")?;
            assert!(s.execute(&format!("UPDATE bank.public.accounts SET {expr} WHERE id=1")).is_err(), "accepted {expr}");
            Ok(())
        }).unwrap();
        assert_eq!(db.last_lsn(), before);
    }
}

#[test]
fn defaults_null_propagation_and_same_type_references_preserve_constraints() {
    let (_dir, root) = migrated();
    let mut db = extended(&root);
    db.execute_ddl("ALTER TABLE bank.public.accounts ALTER balance DROP NOT NULL").unwrap();
    modify(&db, &["UPDATE bank.public.accounts SET note=NULL, copy='changed', other=99, flag=false WHERE id=1",
        "UPDATE bank.public.accounts SET note=DEFAULT, copy=DEFAULT, other=DEFAULT, flag=DEFAULT, balance=NULL+1 WHERE id=1"]);
    assert_eq!(query(&db, "SELECT note,copy,other,flag,balance FROM bank.public.accounts WHERE id=1"),
        vec![vec![Value::String("new".into()), Value::Null, Value::Int64(7), Value::Boolean(true), Value::Null]]);
    modify(&db, &["UPDATE bank.public.accounts SET balance=balance/0, note=copy WHERE id=1"]);
    assert_eq!(query(&db, "SELECT balance,note FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Null, Value::Null]]);
    for expression in ["other=balance+1", "other=NULL+1", "other=DEFAULT+1", "other=+DEFAULT", "small=balance"] {
        db.with_session(|s| {
            s.execute("BEGIN")?;
            assert!(s.execute(&format!("UPDATE bank.public.accounts SET {expression} WHERE id=1")).is_err());
            Ok(())
        }).unwrap();
    }
}

#[test]
fn invalid_binding_and_unsupported_grammar_fail_before_row_mutation() {
    let (_dir, root) = migrated();
    let db = extended(&root);
    let before = db.last_lsn();
    for set in ["id=id+1", "balance=1,balance=2", "balance=missing+1", "balance=small+1",
        "balance=price", "note=note+1", "balance=true+1", "balance='1'+2", "balance=abs(balance)",
        "balance=CASE WHEN true THEN 1 ELSE 0 END", "balance=balance%2", "balance=(1+2", "balance=1+",
        "balance=balance --1", "balance=balance/*comment*/+1", "balance=DEFAULT", "balance=1; DELETE FROM bank.public.audit WHERE id=9"] {
        db.with_session(|s| {
            s.execute("BEGIN")?;
            assert!(s.execute(&format!("UPDATE bank.public.accounts SET {set} WHERE id=1")).is_err(), "accepted {set}");
            Ok(())
        }).unwrap();
        assert_eq!(db.last_lsn(), before);
    }
    db.with_session(|s| {
        s.execute("BEGIN")?;
        assert!(s.execute("UPDATE bank.public.accounts SET balance=missing WHERE id=999").is_err());
        s.execute("BEGIN")?;
        assert_eq!(s.execute("UPDATE bank.public.accounts SET balance=1/0 WHERE id=999")?, SqlResultSet::Command { affected: 0 });
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(), before);
}

#[test]
fn arithmetic_errors_abort_the_whole_statement_and_require_savepoint_recovery() {
    let (_dir, root) = migrated();
    let db = extended(&root);
    let before = db.last_lsn();
    db.with_session(|s| {
        s.execute("BEGIN")?;
        s.execute("UPDATE bank.public.audit SET balance=999 WHERE id=9")?;
        assert!(s.execute("UPDATE bank.public.accounts SET balance=balance+1,other=other/0 WHERE id=1").is_err());
        assert!(s.execute("COMMIT").is_err());
        Ok(())
    }).unwrap();
    assert_eq!(db.last_lsn(), before);
    db.with_session(|s| {
        s.execute("BEGIN WITH READ VALIDATION")?;
        s.execute("UPDATE bank.public.audit SET balance=91 WHERE id=9")?;
        s.execute("SAVEPOINT before_error")?;
        assert!(s.execute("UPDATE bank.public.accounts SET balance=balance+1,other=other/0 WHERE id=1").is_err());
        assert!(s.execute("COMMIT").is_err());
        s.execute("ROLLBACK TO before_error")?;
        assert!(matches!(s.execute("SELECT balance FROM bank.public.accounts WHERE id=1")?,
            SqlResultSet::Rows { rows, .. } if rows == vec![vec![Value::Int64(100)]]));
        s.execute("UPDATE bank.public.accounts SET balance=balance+1 WHERE id=1")?;
        s.execute("COMMIT")?;
        Ok(())
    }).unwrap();
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(101)]]);
    assert_eq!(query(&db, "SELECT balance FROM bank.public.audit WHERE id=9"), vec![vec![Value::Int64(91)]]);
}

#[test]
fn expression_reads_keep_conflict_validation_and_pins() {
    let (_dir, root) = migrated();
    let db = extended(&root);
    db.with_session(|s| {
        s.execute("BEGIN WITH READ VALIDATION")?;
        s.execute("SELECT balance FROM bank.public.accounts WHERE id=2")?;
        s.execute("UPDATE bank.public.accounts SET balance=balance+other WHERE id=1")?;
        modify(&db, &["UPDATE bank.public.accounts SET balance=balance+1 WHERE id=2"]);
        assert!(s.execute("COMMIT").is_err());
        assert!(!s.in_transaction());
        Ok(())
    }).unwrap();
    assert_eq!(query(&db, "SELECT balance FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(100)]]);
    assert_eq!(db.mvcc_stats().active_snapshots, 0);
}

#[test]
fn expression_node_and_nesting_budgets_are_enforced_at_the_boundary() {
    let (_dir, root) = migrated();
    let db = extended(&root);
    for expression in [format!("{}1{}", "(".repeat(32), ")".repeat(32)),
        format!("{}1", "+".repeat(32)), vec!["1";64].join("+")] {
        modify(&db, &[&format!("UPDATE bank.public.accounts SET balance={expression}, other=2 WHERE id=1")]);
    }
    let before = db.last_lsn();
    for expression in [format!("{}1{}", "(".repeat(33), ")".repeat(33)),
        format!("{}1", "+".repeat(33)), vec!["1";65].join("+")] {
        db.with_session(|s| {
            s.execute("BEGIN")?;
            assert!(s.execute(&format!("UPDATE bank.public.accounts SET balance={expression} WHERE id=1")).is_err());
            Ok(())
        }).unwrap();
        assert_eq!(db.last_lsn(), before);
    }
}

#[test]
fn expression_results_survive_history_checkpoint_and_verified_archive_restore() {
    let (dir, root) = migrated();
    let db = extended(&root);
    let before = db.last_lsn();
    modify(&db, &["UPDATE bank.public.accounts SET balance=balance+5, price=price*2.00 WHERE id=1"]);
    let committed = db.last_lsn();
    assert_eq!(query(&db, &format!("SELECT balance,price FROM bank.public.accounts AS OF {before} WHERE id=1")),
        vec![vec![Value::Int64(100), Value::Decimal(125)]]);
    db.checkpoint().unwrap();
    let archive = dir.path().join("expression-backup");
    db.backup_to(&archive).unwrap();
    drop(db);
    let restored = dir.path().join("restored");
    restore_history_database(&archive, &restored).unwrap();
    let db = HistorySqlEngine::open(&restored).unwrap();
    assert_eq!(db.last_lsn(), committed);
    assert_eq!(query(&db, "SELECT balance,price FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Int64(105), Value::Decimal(250)]]);
    modify(&db, &["UPDATE bank.public.accounts SET balance=balance-1 WHERE id=1"]);
}

#[test]
fn older_sql_engine_retains_literal_only_expression_contract() {
    let dir = tempfile::tempdir().unwrap();
    let db = SqlEngine::open(dir.path()).unwrap();
    for sql in ["CREATE DATABASE test", "CREATE SCHEMA test.public",
        "CREATE TABLE test.public.items (id BIGINT NOT NULL, amount BIGINT NOT NULL, PRIMARY KEY(id))",
        "INSERT INTO test.public.items(id,amount) VALUES(-1,-100)"] { db.execute(sql).unwrap(); }
    for set in ["amount=amount+1", "amount=amount-1", "amount=1*2", "amount=DEFAULT"] {
        assert!(db.execute(&format!("UPDATE test.public.items SET {set} WHERE id=-1")).is_err());
    }
    assert!(matches!(db.execute("SELECT amount FROM test.public.items WHERE id=-1").unwrap(),
        SqlResultSet::Rows { rows, .. } if rows == vec![vec![Value::Int64(-100)]]));
}

#[test]
fn fixed_scale_decimal_corpus_matches_small_rational_reference() {
    let (_dir, root) = migrated();
    let db = extended(&root);
    let literal = |coefficient: i128| format!("{}{}.{:02}", if coefficient < 0 { "-" } else { "" },
        coefficient.unsigned_abs()/100, coefficient.unsigned_abs()%100);
    for a in [-225_i128, -100, -1, 0, 1, 25, 100, 250] {
        for b in [-225_i128, -100, -1, 0, 1, 25, 100, 250] {
            for operator in ['+', '-', '*', '/'] {
                // Independent rational reference over small coefficients: no
                // factor cancellation or production expression helpers involved.
                let expected = match operator {
                    '+' => Some(a+b),
                    '-' => Some(a-b),
                    '*' if (a*b)%100 == 0 => Some((a*b)/100),
                    '/' if b != 0 && (a*100)%b == 0 => Some((a*100)/b),
                    _ => None,
                };
                let statement = format!("UPDATE bank.public.accounts SET price={} {} {} WHERE id=1", literal(a), operator, literal(b));
                let before = db.last_lsn();
                db.with_session(|s| {
                    s.execute("BEGIN")?;
                    let result = s.execute(&statement);
                    if expected.is_some() { result?; s.execute("COMMIT")?; }
                    else { assert!(result.is_err(), "accepted {statement}"); assert!(!s.in_transaction()); }
                    Ok(())
                }).unwrap();
                if let Some(expected) = expected {
                    assert_eq!(query(&db, "SELECT price FROM bank.public.accounts WHERE id=1"), vec![vec![Value::Decimal(expected)]], "{statement}");
                } else { assert_eq!(db.last_lsn(), before); }
            }
        }
    }
}
