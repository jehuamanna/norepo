//! Migration #006 + LocalProject repo persistence integration test.

use operon_store::repos::{LocalProjectRepository, SqliteLocalProjectRepository};
use operon_store::{Store, StoreConfig};

#[test]
fn local_project_persists_across_reopen() {
    let tmp = tempfile::NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    // Drop the OS file handle so SQLite can own the path.
    let _ = tmp.as_file();

    let (a_id, b_id, c_id) = {
        let store = Store::open(StoreConfig::local(&path)).unwrap();
        let repo = SqliteLocalProjectRepository::new(store);
        let a = repo.create("alpha").unwrap();
        let b = repo.create("beta").unwrap();
        let c = repo.create("gamma").unwrap();
        (a.id, b.id, c.id)
    };

    {
        let store = Store::open(StoreConfig::local(&path)).unwrap();
        let repo = SqliteLocalProjectRepository::new(store);
        let listed = repo.list().unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].id, a_id);
        assert_eq!(listed[0].name, "alpha");
        assert_eq!(listed[0].sibling_index, 0);
        assert_eq!(listed[1].id, b_id);
        assert_eq!(listed[1].name, "beta");
        assert_eq!(listed[1].sibling_index, 1);
        assert_eq!(listed[2].id, c_id);
        assert_eq!(listed[2].name, "gamma");
        assert_eq!(listed[2].sibling_index, 2);
    }
}
