//! Fixed-iteration comparison harness for this fork against upstream.
//!
//! Release-optimized, no Criterion sample loop. Prints one median per workload.
//!
//! cargo bench -p turso_core --bench fork_compare --profile bench-profile

use std::hint::black_box;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use turso_core::{Connection, Database, PlatformIO, SqliteDialect, Statement, StepResult, Value};

const SCAN_ROWS: i64 = 20_000;
const LOOKUPS: usize = 4_000;
const VECTOR_CALLS: usize = 2_000;
const VECTOR_DIMS: usize = 256;
const INSERT_ROWS: i64 = 5_000;
const SAMPLES: usize = 5;

fn main() {
    let label = std::env::args().nth(1).unwrap_or_else(|| "fork".to_string());
    let scan_db = seed_scan_db();
    let vector_a = vector_text(1.0);
    let vector_b = vector_text(1.25);
    let l2 = scalar(
        &scan_db,
        &format!("SELECT vector_distance_l2('{vector_a}', '{vector_b}')"),
    );
    let cos = scalar(
        &scan_db,
        &format!("SELECT vector_distance_cos('{vector_a}', '{vector_b}')"),
    );

    let scan_one = median(|| time_sql(&scan_db, "SELECT 1 FROM t", SCAN_ROWS as usize));
    let scan_star = median(|| time_sql(&scan_db, "SELECT * FROM t", SCAN_ROWS as usize));
    let lookup = median(|| time_lookups(&scan_db));
    let vector_l2 = median(|| time_vector(&scan_db, "vector_distance_l2", &vector_a, &vector_b));
    let vector_cos = median(|| time_vector(&scan_db, "vector_distance_cos", &vector_a, &vector_b));
    let insert = median(time_inserts);

    println!("label {label}");
    println!("scan_select1_ms {:.3}", scan_one);
    println!("scan_select_star_ms {:.3}", scan_star);
    println!("pk_lookup_{LOOKUPS}_ms {:.3}", lookup);
    println!("vector_l2_{VECTOR_CALLS}x{VECTOR_DIMS}_ms {:.3}", vector_l2);
    println!("vector_cos_{VECTOR_CALLS}x{VECTOR_DIMS}_ms {:.3}", vector_cos);
    println!("insert_{INSERT_ROWS}_ms {:.3}", insert);
    println!("vector_l2_value {l2}");
    println!("vector_cos_value {cos}");
}

struct Db {
    _dir: TempDir,
    db: Arc<Database>,
    conn: Arc<Connection>,
}

fn open_db() -> Db {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("bench.db");
    #[allow(clippy::arc_with_non_send_sync)]
    let io = Arc::new(PlatformIO::new().unwrap());
    let db = Database::open_file(io, path.to_str().unwrap(), Arc::new(SqliteDialect)).unwrap();
    let conn = db.connect().unwrap();
    conn.execute("PRAGMA cache_size=-65536").unwrap();
    Db {
        _dir: dir,
        db,
        conn,
    }
}

fn seed_scan_db() -> Db {
    let db = open_db();
    db.conn
        .execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, value INTEGER NOT NULL)")
        .unwrap();
    db.conn.execute("BEGIN").unwrap();
    let mut insert = db.conn.prepare("INSERT INTO t VALUES (?1, ?2, ?3)").unwrap();
    for i in 0..SCAN_ROWS {
        insert
            .bind_at(1usize.try_into().unwrap(), Value::from_i64(i))
            .unwrap();
        insert
            .bind_at(2usize.try_into().unwrap(), Value::build_text(format!("name_{i}")))
            .unwrap();
        insert
            .bind_at(3usize.try_into().unwrap(), Value::from_i64(i.wrapping_mul(3)))
            .unwrap();
        drive(&db.db, &mut insert);
    }
    db.conn.execute("COMMIT").unwrap();
    db
}

fn time_sql(db: &Db, sql: &str, expect_rows: usize) -> f64 {
    let mut stmt = db.conn.prepare(sql).unwrap();
    let started = Instant::now();
    let rows = drive(&db.db, &mut stmt);
    let elapsed = started.elapsed();
    assert_eq!(rows, expect_rows);
    elapsed_ms(elapsed)
}

fn time_lookups(db: &Db) -> f64 {
    let mut stmt = db
        .conn
        .prepare("SELECT value FROM t WHERE id = ?1")
        .unwrap();
    let started = Instant::now();
    for i in 0..LOOKUPS {
        let id = (i as i64 * 17) % SCAN_ROWS;
        stmt.bind_at(1usize.try_into().unwrap(), Value::from_i64(id))
            .unwrap();
        let rows = drive(&db.db, &mut stmt);
        assert_eq!(rows, 1);
    }
    elapsed_ms(started.elapsed())
}

fn time_vector(db: &Db, func: &str, left: &str, right: &str) -> f64 {
    let mut stmt = db
        .conn
        .prepare(&format!("SELECT {func}(?1, ?2)"))
        .unwrap();
    let started = Instant::now();
    for _ in 0..VECTOR_CALLS {
        stmt.bind_at(1usize.try_into().unwrap(), Value::build_text(left.to_string()))
            .unwrap();
        stmt.bind_at(
            2usize.try_into().unwrap(),
            Value::build_text(right.to_string()),
        )
        .unwrap();
        let rows = drive(&db.db, &mut stmt);
        assert_eq!(rows, 1);
    }
    elapsed_ms(started.elapsed())
}

fn time_inserts() -> f64 {
    let db = open_db();
    db.conn
        .execute("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, value INTEGER NOT NULL)")
        .unwrap();
    let started = Instant::now();
    db.conn.execute("BEGIN").unwrap();
    let mut insert = db.conn.prepare("INSERT INTO t VALUES (?1, ?2, ?3)").unwrap();
    for i in 0..INSERT_ROWS {
        insert
            .bind_at(1usize.try_into().unwrap(), Value::from_i64(i))
            .unwrap();
        insert
            .bind_at(2usize.try_into().unwrap(), Value::build_text(format!("row_{i}")))
            .unwrap();
        insert
            .bind_at(3usize.try_into().unwrap(), Value::from_i64(i))
            .unwrap();
        drive(&db.db, &mut insert);
    }
    db.conn.execute("COMMIT").unwrap();
    elapsed_ms(started.elapsed())
}

fn scalar(db: &Db, sql: &str) -> String {
    let mut stmt = db.conn.prepare(sql).unwrap();
    loop {
        match stmt.step().unwrap() {
            StepResult::Row => {
                let text = format!("{}", stmt.row().unwrap().get_value(0));
                stmt.reset().unwrap();
                return text;
            }
            StepResult::IO | StepResult::Yield | StepResult::Sleep { .. } => {
                db.db.io.step().unwrap();
            }
            StepResult::Done => unreachable!("scalar query returned no row"),
            StepResult::Interrupt | StepResult::Busy => unreachable!(),
        }
    }
}

fn drive(db: &Database, stmt: &mut Statement) -> usize {
    let mut rows = 0;
    loop {
        match stmt.step().unwrap() {
            StepResult::Row => {
                black_box(stmt.row());
                rows += 1;
            }
            StepResult::IO | StepResult::Yield | StepResult::Sleep { .. } => {
                db.io.step().unwrap();
            }
            StepResult::Done => break,
            StepResult::Interrupt | StepResult::Busy => unreachable!(),
        }
    }
    stmt.reset().unwrap();
    rows
}

fn vector_text(scale: f64) -> String {
    let mut body = String::from("[");
    for i in 0..VECTOR_DIMS {
        if i > 0 {
            body.push(',');
        }
        let value = ((i as f64 * scale).sin() * 0.5) + 0.5;
        body.push_str(&format!("{value:.5}"));
    }
    body.push(']');
    body
}

fn median(mut sample: impl FnMut() -> f64) -> f64 {
    let _ = sample();
    let mut values: Vec<f64> = (0..SAMPLES).map(|_| sample()).collect();
    values.sort_by(|a, b| a.partial_cmp(b).unwrap());
    values[values.len() / 2]
}

fn elapsed_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}
