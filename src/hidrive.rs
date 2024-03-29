//! HiDrive access is mediated through the structs in this module.
//!
//! Everywhere you see a `P` type parameter, URL parameters are expected. An easy way to supply
//! them is the `Params` type. You can use other types, though, as long as they serialize to a list
//! of pairs, such as `&[(T0, T1)]` or `BTreeMap<T0, T1>`.
//!

use crate::http::Client;
use crate::oauth2;
use crate::types::*;

use anyhow::{self, Context, Result};
use futures_util::StreamExt;
use hyper::Method;
use log::info;
use reqwest;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_tungstenite::tungstenite::protocol::Message;

pub const NO_BODY: Option<reqwest::Body> = None;
/// Use this if you don't want to supply options to a method. This prevents type errors due to
/// unknown inner type of Option.
pub const NO_PARAMS: Option<&Params> = None;

const DEFAULT_API_BASE_URL: &str = "https://api.hidrive.strato.com/2.1";
const DEFAULT_WS_BASE_URL: &str = "wss://api.hidrive.strato.com/2.1/subscribe";

/// The HiDrive API hub.
///
/// API documentation can be found at
/// [developer.hidrive.com](https://developer.hidrive.com/http-api-reference/).
///
/// All calls are "dynamically typed", taking a collection of parameters varying by call. Check the
/// documentation for which parameters are required for any given call.
pub struct HiDrive {
    client: Client,
    base_url: String,
}

impl HiDrive {
    pub fn new(c: reqwest::Client, a: oauth2::Authorizer) -> HiDrive {
        HiDrive {
            client: Client::new(c, a),
            base_url: DEFAULT_API_BASE_URL.into(),
        }
    }

    pub fn user(&mut self) -> HiDriveUser<'_> {
        HiDriveUser { hd: self }
    }

    pub fn permissions(&mut self) -> HiDrivePermission<'_> {
        HiDrivePermission { hd: self }
    }

    pub fn files(&mut self) -> HiDriveFiles<'_> {
        HiDriveFiles { hd: self }
    }

    pub async fn notifications(&mut self) -> Result<HiDriveNotifications<'_, SecureWSStream>> {
        HiDriveNotifications::new(self, DEFAULT_WS_BASE_URL).await
    }
}

pub struct HiDriveNotifications<'a, S> {
    hd: &'a mut HiDrive,
    stream: tokio_tungstenite::WebSocketStream<S>,
}

type SecureWSStream = tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>;
impl HiDriveNotifications<'_, SecureWSStream> {
    async fn new(
        hd: &mut HiDrive,
        url: impl AsRef<str>,
    ) -> Result<HiDriveNotifications<'_, SecureWSStream>> {
        let url = format!("{}?access_token={}", url.as_ref(), hd.client.access_token().await?);
        info!(target: "hd_api::hidrive", "requesting WSS connection to {}", url);
        tokio_tungstenite::connect_async(url)
            .await
            .map_err(|e| e.into())
            .map(|(stream, _resp)| HiDriveNotifications { hd, stream })
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> HiDriveNotifications<'_, S> {
    pub async fn next(&mut self) -> Result<Option<WebsocketNotification>> {
        loop {
            if let Some(message) = self.stream.next().await {
                match message? {
                    Message::Text(s) => return Ok(serde_json::from_str(s.as_str())?),
                    Message::Close(_) => return Ok(None),
                    _ => continue,
                }
            } else {
                return Ok(None);
            }
        }
    }
}

/// Interact with user information.
pub struct HiDriveUser<'a> {
    hd: &'a mut HiDrive,
}

/// The /user/ API.
///
/// This will be extended in future to allow for administration. For now, it only contains
/// bare-bones features.
impl<'a> HiDriveUser<'a> {
    pub async fn me(&mut self, params: Option<&Params>) -> Result<User> {
        let u = format!("{}/user/me", self.hd.base_url);
        self.hd
            .client
            .request(Method::GET, u, &Params::new(), params)
            .await?
            .go()
            .await
            .context("/user/me")
    }
}

/// Interact with object permissions.
pub struct HiDrivePermission<'a> {
    hd: &'a mut HiDrive,
}

impl<'a> HiDrivePermission<'a> {
    /// GET /permission
    ///
    /// Optional parameters: `pid, account, fields`.
    pub async fn get_permission(
        &mut self,
        id: Identifier,
        p: Option<&Params>,
    ) -> Result<Permissions> {
        let u = format!("{}/permission", self.hd.base_url);
        let mut rqp = Params::new();
        id.to_params(&mut rqp, "pid", "path");
        self.hd
            .client
            .request(Method::GET, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/permission")
    }

    /// PUT /permission
    ///
    /// Optional parameters: `pid, account, invite_id, readable, writable` for P.
    pub async fn set_permission(
        &mut self,
        id: Identifier,
        p: Option<&Params>,
    ) -> Result<Permissions> {
        let u = format!("{}/permission", self.hd.base_url);
        let mut rqp = Params::new();
        id.to_params(&mut rqp, "pid", "path");
        self.hd
            .client
            .request(Method::PUT, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/permission")
    }
}

/// Interact with files.
///
/// Almost all calls identify files or directories by the parameters `pid` (object ID) and `path`
/// (filesystem path).
///
/// * if only `pid` is given, operate on this object.
/// * if only `path` is given, operate on this file or directory.
/// * if both are given, `path` is taken to be relative to `pid`.
///
pub struct HiDriveFiles<'a> {
    hd: &'a mut HiDrive,
}

impl<'a> HiDriveFiles<'a> {
    /// Download file.
    ///
    /// Parameters: `pid, path, snapshot, snaptime`.
    pub async fn get<D: AsyncWrite + Unpin>(
        &mut self,
        id: Identifier,
        out: D,
        p: Option<&Params>,
    ) -> Result<usize> {
        let u = format!("{}/file", self.hd.base_url);
        let mut rqp = Params::new();
        id.to_params(&mut rqp, "pid", "path");
        self.hd
            .client
            .request(Method::GET, u, &rqp, p)
            .await?
            .download_file(out)
            .await
            .context("GET /file")
    }

    /// Obtain a public URL valid for 6 hours.
    ///
    pub async fn url(&mut self, id: Identifier, p: Option<&Params>) -> Result<Url> {
        let u = format!("{}/file/url", self.hd.base_url);
        let mut rqp = Params::new();
        id.to_params(&mut rqp, "pid", "path");
        self.hd
            .client
            .request(Method::GET, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/file/url")
    }

    /// Upload a file (max. 2 gigabytes). Specify either `dir_id`, `dir`, or both; in the latter
    /// case, `dir` is relative to `dir_id`.
    ///
    /// Parameter `name` specifies the file name to be acted on. `dir` or `dir_id` specify the
    /// directory where to create the file. Also available: `mtime, parent_mtime, on_exist`.
    ///
    /// File will not be overwritten if it exists (in that case, code 409 is returned).
    ///
    /// TODO: provide callback for upload status.
    pub async fn upload_no_overwrite<S: AsRef<str>, R: Into<reqwest::Body>>(
        &mut self,
        dir: Identifier,
        name: S,
        src: R,
        p: Option<&Params>,
    ) -> Result<Item> {
        self.upload_(dir, name, src, p, Method::POST).await
    }

    /// Upload a file (max. 2 gigabytes), and overwrite an existing file if it exists.
    ///
    ///
    /// Parameter `name` specifies the file name to be acted on.
    pub async fn upload<S: AsRef<str>, R: Into<reqwest::Body>>(
        &mut self,
        dir: Identifier,
        name: S,
        src: R,
        p: Option<&Params>,
    ) -> Result<Item> {
        self.upload_(dir, name, src, p, Method::PUT).await
    }

    async fn upload_(
        &mut self,
        id: Identifier,
        name: impl AsRef<str>,
        src: impl Into<reqwest::Body>,
        p: Option<&Params>,
        method: Method,
    ) -> Result<Item> {
        let u = format!("{}/file", self.hd.base_url);
        let mut rqp = Params::new();
        id.to_params(&mut rqp, "dir_id", "dir");
        rqp.add_str("name", name.as_ref());
        let method_ = method.clone();
        let ctx = || format!("{} /file", method_);
        self.hd
            .client
            .request(method, u, &rqp, p)
            .await?
            .set_attachment(src)
            .go()
            .await
            .with_context(ctx)
    }

    /// Truncate a file to the specified size. If `size` is greater than the current size, a sparse
    /// file is created.
    pub async fn truncate(
        &mut self,
        id: Identifier,
        size: usize,
        p: Option<&Params>,
    ) -> Result<Item> {
        let u = format!("{}/file/truncate", self.hd.base_url);
        let mut rqp = Params::new();
        rqp.add_uint("size", size);
        id.to_params(&mut rqp, "pid", "path");
        self.hd
            .client
            .request(Method::POST, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/file/truncate")
    }

    /// Copy file.
    ///
    /// Copy from `src` to `dst`. `dst` must be `Path` or `Relative`.
    ///
    /// Also available: `snapshot, snaptime, dst_parent_mtime, preserve_mtime`.
    pub async fn copy(
        &mut self,
        from: Identifier,
        to: Identifier,
        p: Option<&Params>,
    ) -> Result<Item> {
        let u = format!("{}/file/copy", self.hd.base_url);
        let mut rqp = Params::new();
        from.to_params(&mut rqp, "src_id", "src");
        to.to_params(&mut rqp, "dst_id", "dst");
        self.hd
            .client
            .request(Method::POST, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/file/copy")
    }

    /// Move file.
    ///
    /// `to` must be `Relative` or `Path`.
    pub async fn mv(
        &mut self,
        from: Identifier,
        to: Identifier,
        p: Option<&Params>,
    ) -> Result<Item> {
        let u = format!("{}/file/move", self.hd.base_url);
        let mut rqp = Params::new();
        from.to_params(&mut rqp, "src_id", "src");
        to.to_params(&mut rqp, "dst_id", "dst");
        self.hd
            .client
            .request(Method::POST, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/file/move")
    }

    /// Rename operation.
    ///
    /// Takes the new name as required parameter. Useful parameters: `path, pid, on_exist =
    /// {autoname, overwrite}, parent_mtime (int)'.
    pub async fn rename(
        &mut self,
        id: Identifier,
        name: impl AsRef<str>,
        p: Option<&Params>,
    ) -> Result<Item> {
        let u = format!("{}/file/rename", self.hd.base_url);
        let mut rqp = Params::new();
        rqp.add_str("name", name);
        id.to_params(&mut rqp, "pid", "path");
        self.hd
            .client
            .request(Method::GET, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/file/rename")
    }

    /// Delete file.
    pub async fn delete(&mut self, id: Identifier, p: Option<&Params>) -> Result<()> {
        let u = format!("{}/file", self.hd.base_url);
        let mut rqp = Params::new();
        id.to_params(&mut rqp, "pid", "path");
        self.hd
            .client
            .request(Method::DELETE, u, &rqp, p)
            .await?
            .go()
            .await
            .context("DELETE /file")
    }

    /// Download a thumbnail.
    ///
    /// Optional parameters are `width, height, mode, snapshot, snaptime`.
    pub async fn thumbnail<D: AsyncWrite + Unpin>(
        &mut self,
        id: Identifier,
        dst: D,
        p: Option<&Params>,
    ) -> Result<usize> {
        let u = format!("{}/file/thumbnail", self.hd.base_url);
        let mut rqp = Params::new();
        id.to_params(&mut rqp, "pid", "path");
        self.hd
            .client
            .request(Method::GET, u, &rqp, p)
            .await?
            .download_file(dst)
            .await
            .context("/file/thumbnail")
    }

    /// Return metadata. Specify fields to return.
    pub async fn metadata(
        &mut self,
        id: Identifier,
        fields: impl AsRef<str>,
        p: Option<&Params>,
    ) -> Result<Item> {
        let u = format!("{}/meta", self.hd.base_url);
        let mut rqp = Params::new();
        id.to_params(&mut rqp, "pid", "path");
        rqp.add_str("fields", fields);
        self.hd
            .client
            .request(Method::GET, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/meta")
    }

    pub async fn search(
        &mut self,
        root: Identifier,
        fields: impl AsRef<str>,
        p: Option<&Params>,
    ) -> Result<Vec<Item>> {
        let u = format!("{}/search", self.hd.base_url);
        let mut rqp = Params::new();
        root.to_params(&mut rqp, "pid", "path");
        if !fields.as_ref().is_empty() {
            rqp.add_str("fields", fields);
        }
        let r: SearchResult = self
            .hd
            .client
            .request(Method::GET, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/search")?;
        Ok(r.result)
    }

    /// Return metadata for directory.
    ///
    /// Specify either `pid` or `path`, or the request will fail.
    ///
    /// Further parameters: `members, limit, snapshot, snaptime, fields, sort`.
    pub async fn get_dir(&mut self, id: Identifier, p: Option<&Params>) -> Result<Item> {
        let u = format!("{}/dir", self.hd.base_url);
        let mut rqp = Params::new();
        id.to_params(&mut rqp, "pid", "path");
        self.hd
            .client
            .request(Method::GET, u, &rqp, p)
            .await?
            .go()
            .await
            .context("GET /dir")
    }

    /// Return metadata for home directory.
    ///
    /// Further parameters: `members, limit, snapshot, snaptime, fields, sort`.
    pub async fn get_home_dir(&mut self, p: Option<&Params>) -> Result<Item> {
        let u = format!("{}/dir/home", self.hd.base_url);
        self.hd
            .client
            .request(Method::GET, u, &Params::new(), p)
            .await?
            .go()
            .await
            .context("/dir/home")
    }

    /// Create directory.
    ///
    /// `id` must be `Path` or `Relative`.
    ///
    /// Further parameters: `pid, on_exist, mtime, parent_mtime`.
    pub async fn mkdir(&mut self, id: Identifier, p: Option<&Params>) -> Result<Item> {
        let u = format!("{}/dir", self.hd.base_url);
        let mut rqp = Params::new();
        id.to_params(&mut rqp, "pid", "path");
        self.hd
            .client
            .request(Method::POST, u, &rqp, p)
            .await?
            .go()
            .await
            .context("POST /dir")
    }

    /// Remove directory.
    ///
    /// Further parameters: `path, pid, recursive, parent_mtime`.
    pub async fn delete_dir(&mut self, id: Identifier, p: Option<&Params>) -> Result<Item> {
        let u = format!("{}/dir", self.hd.base_url);
        let mut rqp = Params::new();
        id.to_params(&mut rqp, "pid", "path");
        self.hd
            .client
            .request(Method::DELETE, u, &rqp, p)
            .await?
            .go()
            .await
            .context("DELETE /dir")
    }

    /// Copy directory. `to` must be `Relative` or `Path`.
    ///
    /// Further parameters: `on_exist, snapshot, snaptime, dst_parent_mtime,
    /// preserve_mtime`.
    pub async fn copy_dir(
        &mut self,
        from: Identifier,
        to: Identifier,
        p: Option<&Params>,
    ) -> Result<Item> {
        let u = format!("{}/dir/copy", self.hd.base_url);
        let mut rqp = Params::new();
        from.to_params(&mut rqp, "src_id", "src");
        to.to_params(&mut rqp, "dst_id", "dst");
        self.hd
            .client
            .request(Method::POST, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/dir/copy")
    }

    /// Move directory.
    ///
    /// Further parameters: `src, src_id, dst_id, on_exist, src_parent_mtime, dst_parent_mtime,
    /// preserve_mtime`.
    pub async fn mvdir(
        &mut self,
        from: Identifier,
        to: Identifier,
        p: Option<&Params>,
    ) -> Result<Item> {
        let u = format!("{}/dir/move", self.hd.base_url);
        let mut rqp = Params::new();
        from.to_params(&mut rqp, "src_id", "src");
        to.to_params(&mut rqp, "dst_id", "dst");
        self.hd
            .client
            .request(Method::POST, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/dir/move")
    }

    /// Rename directory.
    ///
    /// Takes the new name as required parameter. Useful parameters: `path, pid, on_exist =
    /// {autoname, overwrite}, parent_mtime (int)'.
    pub async fn renamedir(
        &mut self,
        dir: Identifier,
        name: impl AsRef<str>,
        p: Option<&Params>,
    ) -> Result<Item> {
        let u = format!("{}/dir/rename", self.hd.base_url);
        let mut rqp = Params::new();
        rqp.add_str("name", name);
        dir.to_params(&mut rqp, "pid", "path");
        self.hd
            .client
            .request(Method::POST, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/dir/rename")
    }

    /// Get file or directory hash.
    ///
    /// Parameters: `path, pid` (specifying either is mandatory).
    ///
    /// Get hash for given level and ranges. If ranges is empty, return hashes for entire file (but
    /// at most 256).
    pub async fn hash(
        &mut self,
        id: Identifier,
        level: usize,
        ranges: &[(usize, usize)],
        p: Option<&Params>,
    ) -> Result<FileHash> {
        let u = format!("{}/file/hash", self.hd.base_url);
        let mut rqp = Params::new();
        rqp.add_uint("level", level);
        id.to_params(&mut rqp, "pid", "path");
        if ranges.is_empty() {
            rqp.add_str("ranges", "-");
        } else {
            let r = ranges
                .iter()
                .map(|(a, b)| format!("{}-{}", a, b))
                .fold(String::new(), |s, e| (s + ",") + &e);
            rqp.add_str("ranges", &r[1..]);
        }
        self.hd
            .client
            .request(Method::GET, u, &rqp, p)
            .await?
            .go()
            .await
            .context("/file/hash")
    }
}
