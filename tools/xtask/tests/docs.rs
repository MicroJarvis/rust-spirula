use spirula_xtask::{check_docs, workspace_root};

#[test]
fn project_docs_are_consistent() {
    let report = check_docs(&workspace_root()).expect("document check");
    assert!(report.features >= 50, "expected the full feature plan");
    assert!(report.markdown_files >= 10, "expected the docs tree");
    assert!(report.links >= 30, "expected relative links to be checked");
    assert_eq!(report.error_codes, 26, "error catalog size");
}
