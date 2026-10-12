//! Home-local human aliases. Transport identity and peer-owned names never change.
use anyhow::{Result,ensure};
use std::{collections::BTreeMap,path::PathBuf,sync::{LazyLock,Mutex}};
use serde_json::{Value,json};
pub struct Aliases {path:PathBuf,items:BTreeMap<String,String>}
impl Aliases {
 pub fn open(path:PathBuf)->Result<Self>{let items=if path.exists(){serde_json::from_slice(&std::fs::read(&path)?)?}else{BTreeMap::new()};Ok(Self{path,items})}
 pub fn items(&self)->BTreeMap<String,String>{self.items.clone()}
 pub fn set(&mut self,peer:&str,alias:&str)->Result<Value>{
  ensure!(!peer.is_empty()&&peer.len()<=128&&!peer.chars().any(char::is_whitespace),"Invalid peer identity");
  let alias=alias.trim();ensure!(alias.chars().count()<=64&&!alias.chars().any(char::is_control),"Alias must be at most 64 printable characters");
  ensure!(alias.is_empty()||!self.items.iter().any(|(id,name)|id!=peer&&name.to_lowercase()==alias.to_lowercase()),"Another peer already has this alias");
  let mut next=self.items.clone();if alias.is_empty(){next.remove(peer);}else{next.insert(peer.into(),alias.into());}
  ensure!(next.len()<=256,"Too many peer aliases");crate::model::persist(&self.path,&next)?;self.items=next;
  Ok(json!({"tool":"set_peer_alias","peer_id":peer,"alias":alias,"after":{"peer_id":peer,"alias":alias},"summary":if alias.is_empty(){"Removed peer alias.".into()}else{format!("Named peer {alias}.")}}))
 }
}
static ALIASES:LazyLock<Result<Mutex<Aliases>>>=LazyLock::new(||Aliases::open(std::env::var("SPATIAL_PEER_ALIASES").unwrap_or_else(|_|"data/peer-aliases.json".into()).into()).map(Mutex::new));
pub fn all()->Result<BTreeMap<String,String>>{Ok(ALIASES.as_ref().map_err(|e|anyhow::anyhow!("Peer aliases unavailable: {e}"))?.lock().map_err(|_|anyhow::anyhow!("Peer aliases unavailable"))?.items())}
pub fn set(peer:&str,alias:&str)->Result<Value>{ALIASES.as_ref().map_err(|e|anyhow::anyhow!("Peer aliases unavailable: {e}"))?.lock().map_err(|_|anyhow::anyhow!("Peer aliases unavailable"))?.set(peer,alias)}
pub fn name(peer:&str,fallback:&str)->String{all().ok().and_then(|v|v.get(peer).cloned()).unwrap_or_else(||fallback.into())}
pub fn network(mut value:Value)->Result<Value>{let aliases=all()?;if let Some(peers)=value["peers"].as_array_mut(){for peer in peers{if let Some(alias)=peer["peer_id"].as_str().and_then(|id|aliases.get(id)){peer["reported_name"]=peer["name"].clone();peer["name"]=json!(alias);peer["alias"]=json!(alias);}}}value["aliases"]=json!(aliases);Ok(value)}
