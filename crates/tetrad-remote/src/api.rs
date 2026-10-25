//! What the page asks of the device, and what the device answers.

/// The request header that carries the PIN. A custom header also makes a browser send a
/// CORS preflight, which the server does not answer, so other sites cannot post uploads.
pub const PIN_HEADER: &str = "x-tetrad-pin";

/// A request that the device answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    /// The PIN check of the page, and the folder that uploads go to.
    Status { pin: String },
    /// One IR as a stock WAV (mono 48 kHz float32, data chunk at byte 38).
    Upload {
        pin: String,
        name: String,
        wav: Vec<u8>,
    },
    /// Dev build: the running version, and the proof that the unit takes updates.
    #[cfg(feature = "dev")]
    UpdateStatus { pin: String },
    /// Dev build: the update files to send, each as its path under the Tetrad folder and its
    /// size (`bin/tetrad`, `launcher/tetrad-launcher.sh`, ...). Nothing signs them.
    #[cfg(feature = "dev")]
    UpdateBegin {
        pin: String,
        files: Vec<(String, u64)>,
    },
    /// Dev build: the next bytes of manifest file `path`, which must start at `offset`.
    #[cfg(feature = "dev")]
    UpdateChunk {
        pin: String,
        path: String,
        offset: u64,
        bytes: Vec<u8>,
    },
    /// Dev build: every file is here; offer the install on the unit.
    #[cfg(feature = "dev")]
    UpdateReady { pin: String },
    /// Dev build: start the install of the staged files, as UPDATE TETRAD on the unit does. The
    /// unit restarts when it is done.
    #[cfg(feature = "dev")]
    UpdateInstall { pin: String },
    /// Dev build: the recent notices of the unit, as plain text.
    #[cfg(feature = "dev")]
    Log { pin: String },
}

impl Request {
    pub fn pin(&self) -> &str {
        match self {
            Self::Status { pin } | Self::Upload { pin, .. } => pin,
            #[cfg(feature = "dev")]
            Self::UpdateStatus { pin }
            | Self::UpdateBegin { pin, .. }
            | Self::UpdateChunk { pin, .. }
            | Self::UpdateReady { pin }
            | Self::UpdateInstall { pin }
            | Self::Log { pin } => pin,
        }
    }
}

/// One request for the device. `id` matches the [`Reply`] to the open connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub id: u64,
    pub request: Request,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Status {
        folder: String,
    },
    Added {
        name: String,
        folder: String,
    },
    #[cfg(feature = "dev")]
    UpdateStatus {
        version: String,
    },
    /// The unit takes these files: the version it would offer, and the files with their sizes.
    #[cfg(feature = "dev")]
    UpdateBegun {
        version: String,
        files: Vec<(String, u64)>,
    },
    #[cfg(feature = "dev")]
    UpdateReceived {
        path: String,
        received: u64,
    },
    #[cfg(feature = "dev")]
    UpdateStaged {
        version: String,
    },
    /// The install runs; the unit restarts when it ends, so this connection is the last answer.
    #[cfg(feature = "dev")]
    UpdateInstalling,
    /// The notices of the unit, one per line (the answer is `text/plain`).
    #[cfg(feature = "dev")]
    Log {
        text: String,
    },
    WrongPin,
    Off,
    Refused(String),
    /// The device did not answer in time. Only the server makes this reply.
    Busy,
}

impl Reply {
    /// The status code and the JSON body.
    pub(crate) fn response(&self) -> (u16, serde_json::Value) {
        use serde_json::json;
        match self {
            Self::Status { folder } => (200, json!({ "folder": folder })),
            Self::Added { name, folder } => (200, json!({ "name": name, "folder": folder })),
            #[cfg(feature = "dev")]
            Self::UpdateStatus { version } => (200, json!({ "version": version })),
            #[cfg(feature = "dev")]
            Self::UpdateBegun { version, files } => {
                let files: Vec<_> = (files.iter())
                    .map(|(path, size)| json!({ "path": path, "size": size }))
                    .collect();
                (200, json!({ "version": version, "files": files }))
            }
            #[cfg(feature = "dev")]
            Self::UpdateReceived { path, received } => {
                (200, json!({ "path": path, "received": received }))
            }
            #[cfg(feature = "dev")]
            Self::UpdateStaged { version } => (200, json!({ "version": version })),
            #[cfg(feature = "dev")]
            Self::UpdateInstalling => (200, json!({ "installing": true })),
            #[cfg(feature = "dev")]
            Self::Log { text } => (200, json!({ "log": text })),
            Self::WrongPin => (401, json!({ "error": "Wrong PIN" })),
            Self::Off => (503, json!({ "error": "Remote is off" })),
            Self::Refused(reason) => (422, json!({ "error": reason })),
            Self::Busy => (503, json!({ "error": "The unit did not answer" })),
        }
    }
}
