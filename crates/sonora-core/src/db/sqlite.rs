//! Minimal safe wrapper over the SQLite C API.
//!
//! Linking: by default this links the operating system's SQLite
//! (`libsqlite3` on macOS/Linux, `winsqlite3` on Windows 10+), which keeps
//! the workspace buildable offline. The desktop app enables the
//! `external-sqlite` feature and links a bundled, up-to-date SQLite instead.
//!
//! Only the small subset SONORA needs is exposed: prepared statements with
//! positional parameters, typed column access, transactions and batches.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_uchar, c_void, CStr, CString};
use std::fmt;
use std::path::Path;
use std::ptr;

#[repr(C)]
pub struct sqlite3 {
    _private: [u8; 0],
}

#[repr(C)]
pub struct sqlite3_stmt {
    _private: [u8; 0],
}

#[repr(C)]
pub struct sqlite3_context {
    _private: [u8; 0],
}

#[repr(C)]
pub struct sqlite3_value {
    _private: [u8; 0],
}

type ScalarFn = unsafe extern "C" fn(ctx: *mut sqlite3_context, argc: c_int, argv: *mut *mut sqlite3_value);

#[cfg_attr(all(not(feature = "external-sqlite"), not(windows)), link(name = "sqlite3"))]
#[cfg_attr(all(not(feature = "external-sqlite"), windows), link(name = "winsqlite3"))]
extern "C" {
    fn sqlite3_open_v2(filename: *const c_char, db: *mut *mut sqlite3, flags: c_int, vfs: *const c_char) -> c_int;
    fn sqlite3_close_v2(db: *mut sqlite3) -> c_int;
    fn sqlite3_errmsg(db: *mut sqlite3) -> *const c_char;
    fn sqlite3_extended_errcode(db: *mut sqlite3) -> c_int;
    fn sqlite3_busy_timeout(db: *mut sqlite3, ms: c_int) -> c_int;
    fn sqlite3_prepare_v2(db: *mut sqlite3, sql: *const c_char, n: c_int, stmt: *mut *mut sqlite3_stmt, tail: *mut *const c_char) -> c_int;
    fn sqlite3_finalize(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_step(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_bind_int64(stmt: *mut sqlite3_stmt, idx: c_int, v: i64) -> c_int;
    fn sqlite3_bind_double(stmt: *mut sqlite3_stmt, idx: c_int, v: f64) -> c_int;
    fn sqlite3_bind_text(stmt: *mut sqlite3_stmt, idx: c_int, text: *const c_char, n: c_int, destructor: isize) -> c_int;
    fn sqlite3_bind_blob(stmt: *mut sqlite3_stmt, idx: c_int, data: *const c_void, n: c_int, destructor: isize) -> c_int;
    fn sqlite3_bind_null(stmt: *mut sqlite3_stmt, idx: c_int) -> c_int;
    fn sqlite3_bind_parameter_count(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_column_count(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_column_type(stmt: *mut sqlite3_stmt, i: c_int) -> c_int;
    fn sqlite3_column_int64(stmt: *mut sqlite3_stmt, i: c_int) -> i64;
    fn sqlite3_column_double(stmt: *mut sqlite3_stmt, i: c_int) -> f64;
    fn sqlite3_column_text(stmt: *mut sqlite3_stmt, i: c_int) -> *const c_uchar;
    fn sqlite3_column_blob(stmt: *mut sqlite3_stmt, i: c_int) -> *const c_void;
    fn sqlite3_column_bytes(stmt: *mut sqlite3_stmt, i: c_int) -> c_int;
    fn sqlite3_changes(db: *mut sqlite3) -> c_int;
    fn sqlite3_last_insert_rowid(db: *mut sqlite3) -> i64;
    fn sqlite3_libversion() -> *const c_char;
    #[allow(clippy::too_many_arguments)]
    fn sqlite3_create_function_v2(
        db: *mut sqlite3,
        name: *const c_char,
        n_arg: c_int,
        text_rep: c_int,
        app: *mut c_void,
        func: Option<ScalarFn>,
        step: Option<ScalarFn>,
        last: Option<unsafe extern "C" fn(ctx: *mut sqlite3_context)>,
        destroy: Option<unsafe extern "C" fn(app: *mut c_void)>,
    ) -> c_int;
    fn sqlite3_value_type(v: *mut sqlite3_value) -> c_int;
    fn sqlite3_value_text(v: *mut sqlite3_value) -> *const c_uchar;
    fn sqlite3_value_bytes(v: *mut sqlite3_value) -> c_int;
    fn sqlite3_result_text(ctx: *mut sqlite3_context, text: *const c_char, n: c_int, destructor: isize);
    fn sqlite3_result_null(ctx: *mut sqlite3_context);
}

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_NULL: c_int = 5;
const SQLITE_OPEN_READWRITE: c_int = 0x0000_0002;
const SQLITE_OPEN_CREATE: c_int = 0x0000_0004;
const SQLITE_OPEN_NOMUTEX: c_int = 0x0000_8000;
/// `SQLITE_TRANSIENT`: SQLite copies bound text/blobs immediately.
const SQLITE_TRANSIENT: isize = -1;
const SQLITE_UTF8: c_int = 1;
const SQLITE_DETERMINISTIC: c_int = 0x800;

/// `sonora_fold(text)`: the text in lower case, for every script (SQLite's
/// own `lower()` and `LIKE` only know ASCII letters). NULL stays NULL.
unsafe extern "C" fn sonora_fold(ctx: *mut sqlite3_context, argc: c_int, argv: *mut *mut sqlite3_value) {
    if argc != 1 || argv.is_null() {
        sqlite3_result_null(ctx);
        return;
    }
    let v = *argv;
    if sqlite3_value_type(v) == SQLITE_NULL {
        sqlite3_result_null(ctx);
        return;
    }
    // Text first, then its length (the order SQLite documents).
    let p = sqlite3_value_text(v);
    let n = sqlite3_value_bytes(v);
    if p.is_null() || n <= 0 {
        sqlite3_result_text(ctx, c"".as_ptr(), 0, SQLITE_TRANSIENT);
        return;
    }
    let bytes = std::slice::from_raw_parts(p, n as usize);
    let folded = String::from_utf8_lossy(bytes).to_lowercase();
    sqlite3_result_text(ctx, folded.as_ptr() as *const c_char, folded.len() as c_int, SQLITE_TRANSIENT);
}

#[derive(Clone, Debug, PartialEq)]
pub struct DbError {
    pub code: i32,
    pub message: String,
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "database error {}: {}", self.code, self.message)
    }
}

impl std::error::Error for DbError {}

pub type Result<T> = std::result::Result<T, DbError>;

/// Version of the linked SQLite library.
pub fn library_version() -> String {
    unsafe { CStr::from_ptr(sqlite3_libversion()) }.to_string_lossy().to_string()
}

/// A statement whose parameters are being bound. Only this module creates
/// one, from a statement it prepared, so binding through it is safe.
pub struct Binder {
    stmt: *mut sqlite3_stmt,
}

/// A value that can be bound to a statement parameter.
pub trait ToSql {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int;
}

impl ToSql for i64 {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        unsafe { sqlite3_bind_int64(b.stmt, idx, *self) }
    }
}
impl ToSql for i32 {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        (*self as i64).bind_to(b, idx)
    }
}
impl ToSql for u32 {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        (*self as i64).bind_to(b, idx)
    }
}
impl ToSql for u16 {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        (*self as i64).bind_to(b, idx)
    }
}
impl ToSql for u64 {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        (*self as i64).bind_to(b, idx)
    }
}
impl ToSql for usize {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        (*self as i64).bind_to(b, idx)
    }
}
impl ToSql for bool {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        (*self as i64).bind_to(b, idx)
    }
}
impl ToSql for f64 {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        if self.is_finite() {
            unsafe { sqlite3_bind_double(b.stmt, idx, *self) }
        } else {
            unsafe { sqlite3_bind_null(b.stmt, idx) }
        }
    }
}
impl ToSql for str {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        unsafe { sqlite3_bind_text(b.stmt, idx, self.as_ptr() as *const c_char, self.len() as c_int, SQLITE_TRANSIENT) }
    }
}
impl ToSql for String {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        self.as_str().bind_to(b, idx)
    }
}
impl ToSql for [u8] {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        unsafe { sqlite3_bind_blob(b.stmt, idx, self.as_ptr() as *const c_void, self.len() as c_int, SQLITE_TRANSIENT) }
    }
}
impl ToSql for Vec<u8> {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        self.as_slice().bind_to(b, idx)
    }
}
impl<T: ToSql + ?Sized> ToSql for &T {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        (**self).bind_to(b, idx)
    }
}
impl<T: ToSql> ToSql for Option<T> {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        match self {
            Some(v) => v.bind_to(b, idx),
            None => unsafe { sqlite3_bind_null(b.stmt, idx) },
        }
    }
}

/// An owned parameter value, for dynamically built queries.
#[derive(Clone, Debug, PartialEq)]
pub enum SqlValue {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
}

impl ToSql for SqlValue {
    fn bind_to(&self, b: &Binder, idx: c_int) -> c_int {
        match self {
            SqlValue::Null => unsafe { sqlite3_bind_null(b.stmt, idx) },
            SqlValue::Int(v) => v.bind_to(b, idx),
            SqlValue::Real(v) => v.bind_to(b, idx),
            SqlValue::Text(v) => v.bind_to(b, idx),
        }
    }
}

/// Builds a `&[&dyn ToSql]` parameter list.
#[macro_export]
macro_rules! params {
    () => { &[] as &[&dyn $crate::db::sqlite::ToSql] };
    ($($p:expr),+ $(,)?) => { &[$(&$p as &dyn $crate::db::sqlite::ToSql),+] as &[&dyn $crate::db::sqlite::ToSql] };
}

/// A read-only view of the current result row.
pub struct Row<'a> {
    stmt: *mut sqlite3_stmt,
    _marker: std::marker::PhantomData<&'a ()>,
}

impl Row<'_> {
    pub fn is_null(&self, i: usize) -> bool {
        unsafe { sqlite3_column_type(self.stmt, i as c_int) == SQLITE_NULL }
    }
    pub fn i64(&self, i: usize) -> i64 {
        unsafe { sqlite3_column_int64(self.stmt, i as c_int) }
    }
    pub fn opt_i64(&self, i: usize) -> Option<i64> {
        if self.is_null(i) {
            None
        } else {
            Some(self.i64(i))
        }
    }
    pub fn f64(&self, i: usize) -> f64 {
        unsafe { sqlite3_column_double(self.stmt, i as c_int) }
    }
    pub fn opt_f64(&self, i: usize) -> Option<f64> {
        if self.is_null(i) {
            None
        } else {
            Some(self.f64(i))
        }
    }
    pub fn text(&self, i: usize) -> String {
        self.opt_text(i).unwrap_or_default()
    }
    pub fn opt_text(&self, i: usize) -> Option<String> {
        unsafe {
            let p = sqlite3_column_text(self.stmt, i as c_int);
            if p.is_null() {
                return None;
            }
            let n = sqlite3_column_bytes(self.stmt, i as c_int) as usize;
            let bytes = std::slice::from_raw_parts(p, n);
            Some(String::from_utf8_lossy(bytes).into_owned())
        }
    }
    pub fn opt_blob(&self, i: usize) -> Option<Vec<u8>> {
        unsafe {
            if sqlite3_column_type(self.stmt, i as c_int) == SQLITE_NULL {
                return None;
            }
            let p = sqlite3_column_blob(self.stmt, i as c_int);
            let n = sqlite3_column_bytes(self.stmt, i as c_int) as usize;
            if p.is_null() || n == 0 {
                return Some(Vec::new());
            }
            Some(std::slice::from_raw_parts(p as *const u8, n).to_vec())
        }
    }
    pub fn bool(&self, i: usize) -> bool {
        self.i64(i) != 0
    }
    pub fn opt_bool(&self, i: usize) -> Option<bool> {
        self.opt_i64(i).map(|v| v != 0)
    }
    pub fn columns(&self) -> usize {
        unsafe { sqlite3_column_count(self.stmt) as usize }
    }
}

/// A prepared statement.
pub struct Statement<'c> {
    conn: &'c Connection,
    stmt: *mut sqlite3_stmt,
}

impl Drop for Statement<'_> {
    fn drop(&mut self) {
        unsafe {
            sqlite3_finalize(self.stmt);
        }
    }
}

impl<'c> Statement<'c> {
    fn bind(&mut self, params: &[&dyn ToSql]) -> Result<()> {
        let expected = unsafe { sqlite3_bind_parameter_count(self.stmt) } as usize;
        if expected != params.len() {
            return Err(DbError { code: 25, message: format!("statement expects {expected} parameters, got {}", params.len()) });
        }
        for (i, p) in params.iter().enumerate() {
            let rc = p.bind_to(&Binder { stmt: self.stmt }, (i + 1) as c_int);
            if rc != SQLITE_OK {
                return Err(self.conn.error(rc));
            }
        }
        Ok(())
    }

    /// Runs the statement to completion; returns rows changed.
    pub fn execute(&mut self, params: &[&dyn ToSql]) -> Result<usize> {
        self.bind(params)?;
        loop {
            let rc = unsafe { sqlite3_step(self.stmt) };
            match rc {
                SQLITE_DONE => return Ok(unsafe { sqlite3_changes(self.conn.db) } as usize),
                SQLITE_ROW => continue,
                _ => return Err(self.conn.error(rc)),
            }
        }
    }

    /// Maps every result row.
    pub fn query<T>(&mut self, params: &[&dyn ToSql], mut f: impl FnMut(&Row) -> T) -> Result<Vec<T>> {
        self.bind(params)?;
        let mut out = Vec::new();
        loop {
            let rc = unsafe { sqlite3_step(self.stmt) };
            match rc {
                SQLITE_ROW => out.push(f(&Row { stmt: self.stmt, _marker: std::marker::PhantomData })),
                SQLITE_DONE => return Ok(out),
                _ => return Err(self.conn.error(rc)),
            }
        }
    }
}

/// A database connection. Not thread-safe by itself: callers serialise
/// access (the core keeps each connection behind a mutex), which allows the
/// cheaper multi-thread mode of SQLite.
pub struct Connection {
    db: *mut sqlite3,
}

// SAFETY: SQLite in multi-thread mode allows a connection to be used from any
// thread as long as no two threads use it at the same time; the core enforces
// that with a Mutex around every Connection.
unsafe impl Send for Connection {}

impl Drop for Connection {
    fn drop(&mut self) {
        unsafe {
            sqlite3_close_v2(self.db);
        }
    }
}

fn cstring(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| DbError { code: 21, message: "string contains a NUL byte".into() })
}

impl Connection {
    pub fn open(path: &Path) -> Result<Connection> {
        let p = cstring(&path.to_string_lossy())?;
        Self::open_raw(&p)
    }

    pub fn open_in_memory() -> Result<Connection> {
        Self::open_raw(&cstring(":memory:")?)
    }

    fn open_raw(p: &CString) -> Result<Connection> {
        let mut db: *mut sqlite3 = ptr::null_mut();
        let rc = unsafe { sqlite3_open_v2(p.as_ptr(), &mut db, SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_NOMUTEX, ptr::null()) };
        let conn = Connection { db };
        if rc != SQLITE_OK {
            let err = if db.is_null() { DbError { code: rc, message: "could not open database".into() } } else { conn.error(rc) };
            return Err(err);
        }
        unsafe {
            sqlite3_busy_timeout(db, 5000);
        }
        let name = cstring("sonora_fold")?;
        let rc = unsafe { sqlite3_create_function_v2(db, name.as_ptr(), 1, SQLITE_UTF8 | SQLITE_DETERMINISTIC, ptr::null_mut(), Some(sonora_fold), None, None, None) };
        if rc != SQLITE_OK {
            return Err(conn.error(rc));
        }
        Ok(conn)
    }

    fn error(&self, rc: c_int) -> DbError {
        unsafe {
            let msg = CStr::from_ptr(sqlite3_errmsg(self.db)).to_string_lossy().to_string();
            let code = sqlite3_extended_errcode(self.db);
            DbError { code: if code != 0 { code } else { rc }, message: msg }
        }
    }

    pub fn prepare(&self, sql: &str) -> Result<Statement<'_>> {
        let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
        let rc = unsafe { sqlite3_prepare_v2(self.db, sql.as_ptr() as *const c_char, sql.len() as c_int, &mut stmt, ptr::null_mut()) };
        if rc != SQLITE_OK {
            return Err(self.error(rc));
        }
        if stmt.is_null() {
            return Err(DbError { code: 21, message: "empty statement".into() });
        }
        Ok(Statement { conn: self, stmt })
    }

    /// Executes one statement with parameters; returns rows changed.
    pub fn execute(&self, sql: &str, params: &[&dyn ToSql]) -> Result<usize> {
        self.prepare(sql)?.execute(params)
    }

    /// Executes several `;`-separated statements without parameters.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let mut rest = sql.trim();
        while !rest.is_empty() {
            let c = cstring(rest)?;
            let mut stmt: *mut sqlite3_stmt = ptr::null_mut();
            let mut tail: *const c_char = ptr::null();
            let rc = unsafe { sqlite3_prepare_v2(self.db, c.as_ptr(), -1, &mut stmt, &mut tail) };
            if rc != SQLITE_OK {
                return Err(self.error(rc));
            }
            let consumed = if tail.is_null() { rest.len() } else { (unsafe { tail.offset_from(c.as_ptr()) }) as usize };
            if !stmt.is_null() {
                let s = Statement { conn: self, stmt };
                let rc = unsafe { sqlite3_step(s.stmt) };
                if rc != SQLITE_DONE && rc != SQLITE_ROW {
                    return Err(self.error(rc));
                }
            }
            rest = rest[consumed.min(rest.len())..].trim();
        }
        Ok(())
    }

    pub fn query<T>(&self, sql: &str, params: &[&dyn ToSql], f: impl FnMut(&Row) -> T) -> Result<Vec<T>> {
        self.prepare(sql)?.query(params, f)
    }

    pub fn query_row<T>(&self, sql: &str, params: &[&dyn ToSql], f: impl FnMut(&Row) -> T) -> Result<Option<T>> {
        Ok(self.query(sql, params, f)?.into_iter().next())
    }

    pub fn last_insert_rowid(&self) -> i64 {
        unsafe { sqlite3_last_insert_rowid(self.db) }
    }

    /// Runs `f` inside `BEGIN IMMEDIATE … COMMIT`, rolling back on error.
    pub fn transaction<T, E: From<DbError>>(&self, f: impl FnOnce(&Connection) -> std::result::Result<T, E>) -> std::result::Result<T, E> {
        self.execute_batch("BEGIN IMMEDIATE")?;
        match f(self) {
            Ok(v) => {
                if let Err(e) = self.execute_batch("COMMIT") {
                    let _ = self.execute_batch("ROLLBACK");
                    return Err(e.into());
                }
                Ok(v)
            }
            Err(e) => {
                let _ = self.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_types_and_transactions() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT, score REAL, data BLOB, flag INTEGER); CREATE INDEX t_name ON t(name);").unwrap();
        let blob = vec![0u8, 1, 2, 255];
        c.execute("INSERT INTO t (name, score, data, flag) VALUES (?, ?, ?, ?)", params!["héllo 🎧", 91.5, blob, true]).unwrap();
        c.execute("INSERT INTO t (name, score, data, flag) VALUES (?, ?, ?, ?)", params![None::<String>, f64::NAN, None::<Vec<u8>>, false]).unwrap();
        assert_eq!(c.last_insert_rowid(), 2);
        let rows = c.query("SELECT id, name, score, data, flag FROM t ORDER BY id", params![], |r| (r.i64(0), r.opt_text(1), r.opt_f64(2), r.opt_blob(3), r.bool(4))).unwrap();
        assert_eq!(rows[0], (1, Some("héllo 🎧".to_string()), Some(91.5), Some(vec![0, 1, 2, 255]), true));
        assert_eq!(rows[1], (2, None, None, None, false));

        let r: std::result::Result<(), DbError> = c.transaction(|tx| {
            tx.execute("DELETE FROM t", params![])?;
            Err(DbError { code: 1, message: "abort".into() })
        });
        assert!(r.is_err());
        let n = c.query_row("SELECT COUNT(*) FROM t", params![], |r| r.i64(0)).unwrap();
        assert_eq!(n, Some(2), "rolled back");

        let e = c.execute("INSERT INTO nope VALUES (1)", params![]).unwrap_err();
        assert!(e.message.contains("nope"));
        let e = c.execute("SELECT ?", params![]).unwrap_err();
        assert!(e.message.contains("parameters"));
        assert!(!library_version().is_empty());
    }

    #[test]
    fn injection_is_inert() {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch("CREATE TABLE t (name TEXT)").unwrap();
        let evil = "x'); DROP TABLE t; --";
        c.execute("INSERT INTO t VALUES (?)", params![evil]).unwrap();
        let v = c.query_row("SELECT name FROM t", params![], |r| r.text(0)).unwrap().unwrap();
        assert_eq!(v, evil);
    }
}
