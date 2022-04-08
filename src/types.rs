use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiError {
    msg: String,
    code: String,
    auth: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Folder {
    id: String,
    path: String,
    size: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Protocols {
    ftp: bool,
    rsync: bool,
    webdav: bool,
    scp: bool,
    cifs: bool,
    git: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct User {
    account: String,
    encrypted: bool,
    descr: String,
    is_owner: bool,
    email: String,
    email_verified: bool,
    language: String,
    protocols: Protocols,
    is_admin: bool,
    alias: String,
    home: String,
    home_id: String,
    folder: Folder,
}