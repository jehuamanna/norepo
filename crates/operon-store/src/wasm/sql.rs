//! Plans-Phase-2-saving: rusqlite-compatibility shim over `sqlite-wasm-rs`.
//!
//! Covers the subset of the rusqlite API that operon-store's existing
//! repos use (surveyed via `grep` over `crates/operon-store/src/repos/`):
//!
//! - `Connection::{prepare, execute, execute_batch, query_row,
//!   pragma_update, transaction}`
//! - `Statement::{execute, query, query_map, query_row, finalize}` and
//!   the rows iterator returned by `query` / `query_map`
//! - `Row::get<T>(idx)` for `String`, `Option<String>`, `i64`,
//!   `Option<i64>`, `Vec<u8>`, `Option<Vec<u8>>`
//! - `Transaction::{execute, prepare, commit}`; `Drop` rolls back when
//!   not committed
//! - `params!` macro
//! - `OptionalExtension::optional()`
//! - `Error::{SqliteFailure, FromSqlConversionFailure}` and the
//!   `Result<T>` alias
//!
//! Anything outside that surface is intentionally not implemented and
//! returns `Error::Other(String)`.

#![cfg(all(target_arch = "wasm32", feature = "wasm-sqlite"))]

use core::ffi::{c_char, c_int, c_void};
use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::ptr;

use sqlite_wasm_rs as ffi;

// ============================================================================
// Result + Error
// ============================================================================

pub type Result<T> = core::result::Result<T, Error>;

/// Mirrors the rusqlite::Error variants we match on. Callers only inspect
/// `SqliteFailure(extended_code, _)` for unique-constraint detection and
/// `FromSqlConversionFailure(_, Type::Text, _)` for type-conversion errors.
#[derive(Debug)]
pub enum Error {
    SqliteFailure(SqliteFailureInfo, Option<String>),
    FromSqlConversionFailure(usize, Type, Box<dyn std::error::Error + Send + Sync + 'static>),
    Other(String),
}

#[derive(Debug, Clone, Copy)]
pub struct SqliteFailureInfo {
    pub code: i32,
    pub extended_code: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
    Integer,
    Real,
    Text,
    Blob,
    Null,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SqliteFailure(info, msg) => match msg {
                Some(m) => write!(f, "sqlite error code={} ext={}: {}", info.code, info.extended_code, m),
                None => write!(f, "sqlite error code={} ext={}", info.code, info.extended_code),
            },
            Self::FromSqlConversionFailure(col, t, e) => {
                write!(f, "from-sql conversion at column {col} (type {:?}): {e}", t)
            }
            Self::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {}

/// Build an Error::SqliteFailure from a connection's last error.
fn last_error(db: *mut ffi::sqlite3, rc: c_int) -> Error {
    let info = SqliteFailureInfo {
        code: rc as i32,
        extended_code: unsafe { ffi::sqlite3_extended_errcode(db) } as i32,
    };
    let msg = if !db.is_null() {
        let p = unsafe { ffi::sqlite3_errmsg(db) };
        if p.is_null() {
            None
        } else {
            Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
        }
    } else {
        None
    };
    Error::SqliteFailure(info, msg)
}

/// Re-export the constants the desktop `is_unique_violation` matches on,
/// so error.rs can use the same names on both targets via a `crate::sql`
/// module alias (see `sqlite.rs` and `lib.rs`).
pub mod ffi_consts {
    pub const SQLITE_CONSTRAINT_UNIQUE: i32 = 2067;
    pub const SQLITE_CONSTRAINT_PRIMARYKEY: i32 = 1555;
    pub const SQLITE_OK: i32 = 0;
    pub const SQLITE_ROW: i32 = 100;
    pub const SQLITE_DONE: i32 = 101;
}

// ============================================================================
// Connection
// ============================================================================

/// Wrapper around `*mut sqlite3`. Owned; Drop closes the connection.
pub struct Connection {
    db: *mut ffi::sqlite3,
}

unsafe impl Send for Connection {}
unsafe impl Sync for Connection {}

impl Connection {
    /// Construct from an already-opened raw handle. Takes ownership; the
    /// `Connection` will close it on drop.
    pub fn from_raw(db: *mut ffi::sqlite3) -> Self {
        Self { db }
    }

    /// Open a new in-memory connection. Used by the Store wrapper as a
    /// fallback or test path; production opens go through Store::open.
    pub fn open_in_memory() -> Result<Self> {
        let mut db: *mut ffi::sqlite3 = ptr::null_mut();
        let cname = CString::new(":memory:").unwrap();
        let flags = (ffi::SQLITE_OPEN_READWRITE | ffi::SQLITE_OPEN_CREATE) as c_int;
        let rc = unsafe {
            ffi::sqlite3_open_v2(cname.as_ptr(), &mut db as *mut _, flags, ptr::null())
        };
        if rc != ffi_consts::SQLITE_OK as c_int {
            let err = last_error(db, rc);
            unsafe { ffi::sqlite3_close_v2(db) };
            return Err(err);
        }
        Ok(Self { db })
    }

    pub fn raw(&self) -> *mut ffi::sqlite3 {
        self.db
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        let csql = CString::new(sql).map_err(|e| Error::Other(e.to_string()))?;
        let rc = unsafe {
            ffi::sqlite3_exec(self.db, csql.as_ptr(), None, ptr::null_mut(), ptr::null_mut())
        };
        if rc != ffi_consts::SQLITE_OK as c_int {
            return Err(last_error(self.db, rc));
        }
        Ok(())
    }

    pub fn execute(&self, sql: &str, params: &[&dyn ToSql]) -> Result<usize> {
        let mut stmt = self.prepare(sql)?;
        stmt.execute(params)
    }

    pub fn prepare<'a>(&'a self, sql: &str) -> Result<Statement<'a>> {
        Statement::prepare(self.db, sql)
    }

    pub fn query_row<T, F>(&self, sql: &str, params: &[&dyn ToSql], f: F) -> Result<T>
    where
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        let mut stmt = self.prepare(sql)?;
        stmt.query_row(params, f)
    }

    /// Mirrors `rusqlite::Connection::pragma_update(None, key, value)`.
    /// Schemas other than `None` aren't used by operon-store today.
    pub fn pragma_update(&self, _schema: Option<&str>, name: &str, value: &str) -> Result<()> {
        // Pragma values are not bindable in standard SQLite; we
        // interpolate. Validate the name to keep the surface tight.
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::Other(format!("invalid pragma name: {name}")));
        }
        if !value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(Error::Other(format!("invalid pragma value: {value}")));
        }
        self.execute_batch(&format!("PRAGMA {name} = {value}"))
    }

    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        self.execute_batch("BEGIN")?;
        Ok(Transaction {
            conn: self,
            committed: false,
        })
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if !self.db.is_null() {
            unsafe { ffi::sqlite3_close_v2(self.db) };
        }
    }
}

// ============================================================================
// Statement
// ============================================================================

pub struct Statement<'conn> {
    stmt: *mut ffi::sqlite3_stmt,
    db: *mut ffi::sqlite3,
    _phantom: PhantomData<&'conn ()>,
}

impl<'conn> Statement<'conn> {
    fn prepare(db: *mut ffi::sqlite3, sql: &str) -> Result<Self> {
        let csql = CString::new(sql).map_err(|e| Error::Other(e.to_string()))?;
        let mut stmt: *mut ffi::sqlite3_stmt = ptr::null_mut();
        let rc = unsafe {
            ffi::sqlite3_prepare_v2(
                db,
                csql.as_ptr(),
                -1,
                &mut stmt as *mut _,
                ptr::null_mut(),
            )
        };
        if rc != ffi_consts::SQLITE_OK as c_int {
            return Err(last_error(db, rc));
        }
        Ok(Self {
            stmt,
            db,
            _phantom: PhantomData,
        })
    }

    fn bind_all(&mut self, params: &[&dyn ToSql]) -> Result<()> {
        unsafe { ffi::sqlite3_reset(self.stmt) };
        unsafe { ffi::sqlite3_clear_bindings(self.stmt) };
        for (i, p) in params.iter().enumerate() {
            // SQLite bind indexes are 1-based.
            p.bind(self.stmt, (i + 1) as c_int).map_err(|e| {
                Error::Other(format!("bind param {}: {}", i + 1, e))
            })?;
        }
        Ok(())
    }

    /// Execute a non-row-returning statement. Returns the number of
    /// changes (rows affected) per `sqlite3_changes`.
    pub fn execute(&mut self, params: &[&dyn ToSql]) -> Result<usize> {
        self.bind_all(params)?;
        let rc = unsafe { ffi::sqlite3_step(self.stmt) };
        if rc != ffi_consts::SQLITE_DONE as c_int && rc != ffi_consts::SQLITE_ROW as c_int {
            return Err(last_error(self.db, rc));
        }
        let n = unsafe { ffi::sqlite3_changes(self.db) };
        Ok(n as usize)
    }

    pub fn query<'s>(&'s mut self, params: &[&dyn ToSql]) -> Result<Rows<'s, 'conn>> {
        self.bind_all(params)?;
        Ok(Rows { stmt: self })
    }

    pub fn query_map<T, F>(&mut self, params: &[&dyn ToSql], mut f: F) -> Result<Vec<Result<T>>>
    where
        F: FnMut(&Row<'_>) -> Result<T>,
    {
        // rusqlite returns an iterator; we materialize the Vec eagerly to
        // keep lifetimes simple. Repos that needed streaming behaviour
        // (none today) would need a refactor.
        self.bind_all(params)?;
        let mut out = Vec::new();
        loop {
            let rc = unsafe { ffi::sqlite3_step(self.stmt) };
            if rc == ffi_consts::SQLITE_DONE as c_int {
                break;
            }
            if rc != ffi_consts::SQLITE_ROW as c_int {
                return Err(last_error(self.db, rc));
            }
            let row = Row {
                stmt: self.stmt,
                _phantom: PhantomData,
            };
            out.push(f(&row));
        }
        Ok(out)
    }

    pub fn query_row<T, F>(&mut self, params: &[&dyn ToSql], f: F) -> Result<T>
    where
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        self.bind_all(params)?;
        let rc = unsafe { ffi::sqlite3_step(self.stmt) };
        if rc == ffi_consts::SQLITE_ROW as c_int {
            let row = Row {
                stmt: self.stmt,
                _phantom: PhantomData,
            };
            f(&row)
        } else if rc == ffi_consts::SQLITE_DONE as c_int {
            // rusqlite returns Error::QueryReturnedNoRows; we collapse to
            // a NotFound-flavored Error::Other since `is_unique_violation`
            // is the only matched-on variant.
            Err(Error::Other("query returned no rows".into()))
        } else {
            Err(last_error(self.db, rc))
        }
    }
}

impl Drop for Statement<'_> {
    fn drop(&mut self) {
        if !self.stmt.is_null() {
            unsafe { ffi::sqlite3_finalize(self.stmt) };
        }
    }
}

/// `Statement::query` return type. Implements an iterator of `Result<Row>`.
pub struct Rows<'s, 'conn: 's> {
    stmt: &'s mut Statement<'conn>,
}

impl<'s, 'conn> Rows<'s, 'conn> {
    pub fn next(&mut self) -> Result<Option<Row<'_>>> {
        let rc = unsafe { ffi::sqlite3_step(self.stmt.stmt) };
        if rc == ffi_consts::SQLITE_DONE as c_int {
            Ok(None)
        } else if rc == ffi_consts::SQLITE_ROW as c_int {
            Ok(Some(Row {
                stmt: self.stmt.stmt,
                _phantom: PhantomData,
            }))
        } else {
            Err(last_error(self.stmt.db, rc))
        }
    }
}

// ============================================================================
// Row + FromSql
// ============================================================================

pub struct Row<'a> {
    stmt: *mut ffi::sqlite3_stmt,
    _phantom: PhantomData<&'a ()>,
}

impl<'a> Row<'a> {
    pub fn get<T: FromSql>(&self, idx: usize) -> Result<T> {
        T::from_column(self.stmt, idx as c_int)
    }
}

pub trait FromSql: Sized {
    fn from_column(stmt: *mut ffi::sqlite3_stmt, col: c_int) -> Result<Self>;
}

fn col_type(stmt: *mut ffi::sqlite3_stmt, col: c_int) -> Type {
    let t = unsafe { ffi::sqlite3_column_type(stmt, col) };
    match t {
        ffi::SQLITE_INTEGER => Type::Integer,
        ffi::SQLITE_FLOAT => Type::Real,
        ffi::SQLITE_TEXT => Type::Text,
        ffi::SQLITE_BLOB => Type::Blob,
        _ => Type::Null,
    }
}

impl FromSql for String {
    fn from_column(stmt: *mut ffi::sqlite3_stmt, col: c_int) -> Result<Self> {
        if matches!(col_type(stmt, col), Type::Null) {
            return Err(Error::FromSqlConversionFailure(
                col as usize,
                Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "expected non-null TEXT",
                )),
            ));
        }
        let p = unsafe { ffi::sqlite3_column_text(stmt, col) } as *const c_char;
        if p.is_null() {
            return Ok(String::new());
        }
        let n = unsafe { ffi::sqlite3_column_bytes(stmt, col) } as usize;
        let bytes = unsafe { core::slice::from_raw_parts(p as *const u8, n) };
        std::str::from_utf8(bytes)
            .map(|s| s.to_owned())
            .map_err(|e| {
                Error::FromSqlConversionFailure(col as usize, Type::Text, Box::new(e))
            })
    }
}

impl FromSql for Option<String> {
    fn from_column(stmt: *mut ffi::sqlite3_stmt, col: c_int) -> Result<Self> {
        if matches!(col_type(stmt, col), Type::Null) {
            Ok(None)
        } else {
            String::from_column(stmt, col).map(Some)
        }
    }
}

impl FromSql for i64 {
    fn from_column(stmt: *mut ffi::sqlite3_stmt, col: c_int) -> Result<Self> {
        Ok(unsafe { ffi::sqlite3_column_int64(stmt, col) })
    }
}

impl FromSql for Option<i64> {
    fn from_column(stmt: *mut ffi::sqlite3_stmt, col: c_int) -> Result<Self> {
        if matches!(col_type(stmt, col), Type::Null) {
            Ok(None)
        } else {
            Ok(Some(unsafe { ffi::sqlite3_column_int64(stmt, col) }))
        }
    }
}

impl FromSql for i32 {
    fn from_column(stmt: *mut ffi::sqlite3_stmt, col: c_int) -> Result<Self> {
        Ok(unsafe { ffi::sqlite3_column_int(stmt, col) } as i32)
    }
}

impl FromSql for Vec<u8> {
    fn from_column(stmt: *mut ffi::sqlite3_stmt, col: c_int) -> Result<Self> {
        if matches!(col_type(stmt, col), Type::Null) {
            return Ok(Vec::new());
        }
        let p = unsafe { ffi::sqlite3_column_blob(stmt, col) } as *const u8;
        let n = unsafe { ffi::sqlite3_column_bytes(stmt, col) } as usize;
        if p.is_null() {
            return Ok(Vec::new());
        }
        Ok(unsafe { core::slice::from_raw_parts(p, n) }.to_vec())
    }
}

impl FromSql for Option<Vec<u8>> {
    fn from_column(stmt: *mut ffi::sqlite3_stmt, col: c_int) -> Result<Self> {
        if matches!(col_type(stmt, col), Type::Null) {
            Ok(None)
        } else {
            Vec::<u8>::from_column(stmt, col).map(Some)
        }
    }
}

impl FromSql for bool {
    fn from_column(stmt: *mut ffi::sqlite3_stmt, col: c_int) -> Result<Self> {
        Ok(unsafe { ffi::sqlite3_column_int(stmt, col) } != 0)
    }
}

// ============================================================================
// ToSql
// ============================================================================

/// `params!` produces a slice of these. Each impl knows how to call the
/// right `sqlite3_bind_*` function for its concrete Rust type.
pub trait ToSql {
    fn bind(&self, stmt: *mut ffi::sqlite3_stmt, idx: c_int)
        -> core::result::Result<(), String>;
}

const SQLITE_TRANSIENT: ffi::sqlite3_destructor_type = unsafe {
    core::mem::transmute::<isize, ffi::sqlite3_destructor_type>(-1)
};

fn check_bind(rc: c_int) -> core::result::Result<(), String> {
    if rc == ffi_consts::SQLITE_OK as c_int {
        Ok(())
    } else {
        Err(format!("sqlite3_bind rc={rc}"))
    }
}

impl ToSql for &str {
    fn bind(&self, stmt: *mut ffi::sqlite3_stmt, idx: c_int)
        -> core::result::Result<(), String>
    {
        let bytes = self.as_bytes();
        check_bind(unsafe {
            ffi::sqlite3_bind_text(
                stmt,
                idx,
                bytes.as_ptr() as *const c_char,
                bytes.len() as c_int,
                SQLITE_TRANSIENT,
            )
        })
    }
}

impl ToSql for String {
    fn bind(&self, stmt: *mut ffi::sqlite3_stmt, idx: c_int)
        -> core::result::Result<(), String>
    {
        self.as_str().bind(stmt, idx)
    }
}

impl ToSql for &String {
    fn bind(&self, stmt: *mut ffi::sqlite3_stmt, idx: c_int)
        -> core::result::Result<(), String>
    {
        self.as_str().bind(stmt, idx)
    }
}

impl ToSql for i64 {
    fn bind(&self, stmt: *mut ffi::sqlite3_stmt, idx: c_int)
        -> core::result::Result<(), String>
    {
        check_bind(unsafe { ffi::sqlite3_bind_int64(stmt, idx, *self) })
    }
}

impl ToSql for i32 {
    fn bind(&self, stmt: *mut ffi::sqlite3_stmt, idx: c_int)
        -> core::result::Result<(), String>
    {
        check_bind(unsafe { ffi::sqlite3_bind_int(stmt, idx, *self) })
    }
}

impl ToSql for usize {
    fn bind(&self, stmt: *mut ffi::sqlite3_stmt, idx: c_int)
        -> core::result::Result<(), String>
    {
        (*self as i64).bind(stmt, idx)
    }
}

impl ToSql for bool {
    fn bind(&self, stmt: *mut ffi::sqlite3_stmt, idx: c_int)
        -> core::result::Result<(), String>
    {
        check_bind(unsafe { ffi::sqlite3_bind_int(stmt, idx, if *self { 1 } else { 0 }) })
    }
}

impl ToSql for &[u8] {
    fn bind(&self, stmt: *mut ffi::sqlite3_stmt, idx: c_int)
        -> core::result::Result<(), String>
    {
        check_bind(unsafe {
            ffi::sqlite3_bind_blob(
                stmt,
                idx,
                self.as_ptr() as *const c_void,
                self.len() as c_int,
                SQLITE_TRANSIENT,
            )
        })
    }
}

impl ToSql for Vec<u8> {
    fn bind(&self, stmt: *mut ffi::sqlite3_stmt, idx: c_int)
        -> core::result::Result<(), String>
    {
        self.as_slice().bind(stmt, idx)
    }
}

impl<T: ToSql + ?Sized> ToSql for &T {
    fn bind(&self, stmt: *mut ffi::sqlite3_stmt, idx: c_int)
        -> core::result::Result<(), String>
    {
        (**self).bind(stmt, idx)
    }
}

impl<T: ToSql> ToSql for Option<T> {
    fn bind(&self, stmt: *mut ffi::sqlite3_stmt, idx: c_int)
        -> core::result::Result<(), String>
    {
        match self {
            Some(v) => v.bind(stmt, idx),
            None => check_bind(unsafe { ffi::sqlite3_bind_null(stmt, idx) }),
        }
    }
}

/// Helper used by the `params!` macro.
pub fn params_slice<'a>(items: &'a [&'a dyn ToSql]) -> &'a [&'a dyn ToSql] {
    items
}

/// Mirrors rusqlite::params!. Repos pass this to `prepare`/`execute`/
/// `query_map`/`query_row`.
#[macro_export]
macro_rules! params {
    () => { &[] as &[&dyn $crate::wasm::sql::ToSql] };
    ($($e:expr),+ $(,)?) => {
        &[$( &$e as &dyn $crate::wasm::sql::ToSql ),+] as &[&dyn $crate::wasm::sql::ToSql]
    };
}

// ============================================================================
// Transaction
// ============================================================================

pub struct Transaction<'conn> {
    conn: &'conn Connection,
    committed: bool,
}

impl<'conn> Transaction<'conn> {
    pub fn execute(&self, sql: &str, params: &[&dyn ToSql]) -> Result<usize> {
        self.conn.execute(sql, params)
    }

    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        self.conn.execute_batch(sql)
    }

    pub fn prepare<'a>(&'a self, sql: &str) -> Result<Statement<'a>> {
        self.conn.prepare(sql)
    }

    pub fn query_row<T, F>(&self, sql: &str, params: &[&dyn ToSql], f: F) -> Result<T>
    where
        F: FnOnce(&Row<'_>) -> Result<T>,
    {
        self.conn.query_row(sql, params, f)
    }

    pub fn commit(mut self) -> Result<()> {
        self.conn.execute_batch("COMMIT")?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for Transaction<'_> {
    fn drop(&mut self) {
        if !self.committed {
            // Best-effort rollback. Errors here are silently swallowed —
            // mirrors rusqlite's behaviour.
            let _ = self.conn.execute_batch("ROLLBACK");
        }
    }
}

// ============================================================================
// OptionalExtension
// ============================================================================

/// Mirrors `rusqlite::OptionalExtension`. Repos call `.optional()` on a
/// `Result<T>` to convert the "no rows" error into `None`.
pub trait OptionalExtension<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExtension<T> for Result<T> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(Error::Other(s)) if s == "query returned no rows" => Ok(None),
            Err(e) => Err(e),
        }
    }
}
