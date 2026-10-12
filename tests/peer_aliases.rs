use spatial_home::peer_aliases::Aliases;
#[test]
fn aliases_persist_preserve_identity_and_reject_duplicate_names()->anyhow::Result<()>{
 let dir=tempfile::tempdir()?;let path=dir.path().join("aliases.json");let mut aliases=Aliases::open(path.clone())?;
 aliases.set("peer-a","Dan's phone")?;assert_eq!(Aliases::open(path.clone())?.items()["peer-a"],"Dan's phone");
 assert!(aliases.set("peer-b","dan's phone").is_err());assert!(aliases.set("peer-b","bad\nname").is_err());assert!(!aliases.items().contains_key("peer-b"));
 aliases.set("peer-a","")?;assert!(Aliases::open(path)?.items().is_empty());Ok(())
}
