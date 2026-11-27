use bytes::BytesMut;
use std::fmt::Debug;
use std::io::Write;
use steam_vent::message::{EncodableMessage, MalformedBody, NetMessage};
use steam_vent::{
    Connection, ConnectionTrait, NetMessageHeader, NetworkError, ServiceMethodRequest,
};
use steam_vent_proto_common::protobuf::{CodedInputStream, CodedOutputStream};
use steam_vent_proto_common::RpcMessage;
use steam_vent_proto_steam::enums_clientserver::EMsg;

const MAX_SANITIZE_DEPTH: usize = 32;

const WIRE_TYPE_VARINT: u32 = 0;
const WIRE_TYPE_FIXED64: u32 = 1;
const WIRE_TYPE_LENGTH_DELIMITED: u32 = 2;
const WIRE_TYPE_FIXED32: u32 = 5;

#[derive(Clone, Copy)]
pub(super) enum FieldSpec {
    String,
    Message(&'static MessageSchema),
}

pub(super) struct MessageSchema {
    fields: &'static [(u32, FieldSpec)],
}

impl MessageSchema {
    fn get(&self, field_number: u32) -> Option<FieldSpec> {
        self.fields
            .iter()
            .find_map(|(num, spec)| (*num == field_number).then_some(*spec))
    }
}

static PUBLISHED_FILE_PREVIEW_SCHEMA: MessageSchema = MessageSchema {
    fields: &[
        (3, FieldSpec::String), // url
        (5, FieldSpec::String), // filename
        (6, FieldSpec::String), // youtubevideoid
        (8, FieldSpec::String), // external_reference
    ],
};

static PUBLISHED_FILE_TAG_SCHEMA: MessageSchema = MessageSchema {
    fields: &[
        (1, FieldSpec::String), // tag
        (3, FieldSpec::String), // display_name
    ],
};

static PUBLISHED_FILE_KVTAG_SCHEMA: MessageSchema = MessageSchema {
    fields: &[
        (1, FieldSpec::String), // key
        (2, FieldSpec::String), // value
    ],
};

static PUBLISHED_FILE_AUTHOR_SNAPSHOT_SCHEMA: MessageSchema = MessageSchema {
    fields: &[
        (2, FieldSpec::String), // game_branch_min
        (3, FieldSpec::String), // game_branch_max
    ],
};

static PUBLISHED_FILE_DETAILS_SCHEMA: MessageSchema = MessageSchema {
    fields: &[
        (7, FieldSpec::String),                                   // filename
        (10, FieldSpec::String),                                  // file_url
        (11, FieldSpec::String),                                  // preview_url
        (12, FieldSpec::String),                                  // youtubevideoid
        (13, FieldSpec::String),                                  // url
        (16, FieldSpec::String),                                  // title
        (17, FieldSpec::String),                                  // file_description
        (18, FieldSpec::String),                                  // short_description
        (29, FieldSpec::String),                                  // ban_reason
        (33, FieldSpec::String),                                  // app_name
        (45, FieldSpec::String),                                  // image_url
        (48, FieldSpec::String),                                  // shortcutname
        (51, FieldSpec::Message(&PUBLISHED_FILE_PREVIEW_SCHEMA)), // previews
        (52, FieldSpec::Message(&PUBLISHED_FILE_TAG_SCHEMA)),     // tags
        (54, FieldSpec::Message(&PUBLISHED_FILE_KVTAG_SCHEMA)),   // kvtags
        (58, FieldSpec::String),                                  // metadata
        (
            75,
            FieldSpec::Message(&PUBLISHED_FILE_AUTHOR_SNAPSHOT_SCHEMA),
        ), // author_snapshots
    ],
};

pub(super) static QUERY_FILES_RESPONSE_SCHEMA: &MessageSchema = &MessageSchema {
    fields: &[
        (2, FieldSpec::Message(&PUBLISHED_FILE_DETAILS_SCHEMA)), // publishedfiledetails
        (3, FieldSpec::String),                                  // next_cursor
    ],
};

pub(super) static GET_DETAILS_RESPONSE_SCHEMA: &MessageSchema = &MessageSchema {
    fields: &[
        (1, FieldSpec::Message(&PUBLISHED_FILE_DETAILS_SCHEMA)), // publishedfiledetails
    ],
};

static ACCOUNT_PUBLIC_DATA_SCHEMA: MessageSchema = MessageSchema {
    fields: &[
        // Field 9 (sha_digest_avatar) is `bytes`, intentionally omitted so raw binary is untouched.
        (10, FieldSpec::String), // persona_name
        (11, FieldSpec::String), // profile_url
    ],
};

static ACCOUNT_PRIVATE_DATA_SCHEMA: MessageSchema = MessageSchema {
    fields: &[
        (8, FieldSpec::String),  // game_extra_info
        (9, FieldSpec::String),  // account_name
        (11, FieldSpec::String), // rich_presence_kv
        (16, FieldSpec::String), // watching_broadcast_title
        (21, FieldSpec::String), // game_device_name
    ],
};

static PLAYER_LINK_DETAILS_SCHEMA: MessageSchema = MessageSchema {
    fields: &[
        (1, FieldSpec::Message(&ACCOUNT_PUBLIC_DATA_SCHEMA)), // public_data
        (2, FieldSpec::Message(&ACCOUNT_PRIVATE_DATA_SCHEMA)), // private_data
    ],
};

pub(super) static GET_PLAYER_LINK_DETAILS_RESPONSE_SCHEMA: &MessageSchema = &MessageSchema {
    fields: &[
        (1, FieldSpec::Message(&PLAYER_LINK_DETAILS_SCHEMA)), // accounts
    ],
};

#[derive(Debug)]
pub(super) struct RawServiceMethodRequest<Request: Debug>(pub(super) Request);

impl<Request: ServiceMethodRequest + Debug> EncodableMessage for RawServiceMethodRequest<Request> {
    fn write_body<W: Write>(&self, mut writer: W) -> Result<(), std::io::Error> {
        self.0
            .write(&mut writer)
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidData))
    }

    fn encode_size(&self) -> usize {
        self.0.compute_size() as usize
    }

    fn process_header(&self, header: &mut NetMessageHeader) {
        header.target_job_name = Some(Request::REQ_NAME.into());
    }
}

impl<Request: ServiceMethodRequest + Debug> NetMessage for RawServiceMethodRequest<Request> {
    type KindEnum = EMsg;
    const KIND: Self::KindEnum = EMsg::k_EMsgServiceMethodCallFromClient;
    const IS_PROTOBUF: bool = true;
}

#[derive(Debug)]
pub(super) struct RawServiceMethodResponse {
    pub(super) job_name: String,
    pub(super) body: BytesMut,
}

impl RawServiceMethodResponse {
    pub(super) fn into_response<Request: ServiceMethodRequest>(
        self,
        schema: &MessageSchema,
    ) -> Result<Request::Response, NetworkError> {
        if self.job_name != Request::REQ_NAME {
            return Err(NetworkError::DifferentServiceMethod(
                Request::REQ_NAME,
                self.job_name,
            ));
        }
        decode_response_lossy::<Request::Response>(Request::REQ_NAME, &self.body, schema)
    }
}

impl EncodableMessage for RawServiceMethodResponse {
    fn read_body(data: BytesMut, header: &NetMessageHeader) -> Result<Self, MalformedBody> {
        Ok(Self {
            job_name: header
                .target_job_name
                .as_deref()
                .unwrap_or_default()
                .to_string(),
            body: data,
        })
    }
}

impl NetMessage for RawServiceMethodResponse {
    type KindEnum = EMsg;
    const KIND: Self::KindEnum = EMsg::k_EMsgServiceMethodResponse;
    const IS_PROTOBUF: bool = true;
}

/// Calls a Steam CM service method while tolerating invalid UTF-8 sequences that Steam's C++
/// proto2 backend occasionally emits inside `string` fields.
pub(super) async fn service_method_lossy<Request: ServiceMethodRequest>(
    conn: &Connection,
    request: Request,
    schema: &'static MessageSchema,
) -> Result<Request::Response, NetworkError> {
    let response: RawServiceMethodResponse = conn.job(RawServiceMethodRequest(request)).await?;
    response.into_response::<Request>(schema)
}

#[derive(Default)]
struct SanitizeStats {
    repaired_fields: usize,
}

fn is_utf8_protobuf_error(err: &steam_vent_proto_common::protobuf::Error) -> bool {
    let msg = err.to_string();
    msg.contains("UTF-8")
}

pub(super) fn decode_response_lossy<Response: RpcMessage>(
    method_name: &'static str,
    raw: &[u8],
    schema: &MessageSchema,
) -> Result<Response, NetworkError> {
    let mut slice = raw;
    match Response::parse(&mut slice) {
        Ok(parsed) => Ok(parsed),
        Err(original_err) if is_utf8_protobuf_error(&original_err) => {
            let mut stats = SanitizeStats::default();
            let sanitized = match sanitize_message_bytes(raw, schema, 0, &mut stats) {
                Ok(bytes) if stats.repaired_fields > 0 => bytes,
                Ok(_) => {
                    tracing::warn!(
                        method = method_name,
                        error = %original_err,
                        "Steam CM response triggered a UTF-8 decode error, but no invalid bytes were found in known string fields (possibly an unhandled new field)"
                    );
                    return Err(MalformedBody::new(
                        EMsg::k_EMsgServiceMethodResponse,
                        original_err,
                    )
                    .into());
                }
                Err(sanitize_err) => {
                    tracing::warn!(
                        method = method_name,
                        error = %original_err,
                        sanitize_error = %sanitize_err,
                        "Steam CM response UTF-8 sanitization failed; keeping original parse error"
                    );
                    return Err(MalformedBody::new(
                        EMsg::k_EMsgServiceMethodResponse,
                        original_err,
                    )
                    .into());
                }
            };
            tracing::warn!(
                method = method_name,
                repaired_fields = stats.repaired_fields,
                "Steam CM response contained invalid UTF-8 bytes; sanitized lossily and continued parsing"
            );
            let mut sanitized_slice = sanitized.as_slice();
            Response::parse(&mut sanitized_slice)
                .map_err(|err| MalformedBody::new(EMsg::k_EMsgServiceMethodResponse, err).into())
        }
        Err(err) => Err(MalformedBody::new(EMsg::k_EMsgServiceMethodResponse, err).into()),
    }
}

fn sanitize_message_bytes(
    raw: &[u8],
    schema: &MessageSchema,
    depth: usize,
    stats: &mut SanitizeStats,
) -> Result<Vec<u8>, String> {
    if depth > MAX_SANITIZE_DEPTH {
        return Err("protobuf nesting depth exceeded".to_string());
    }
    let mut is = CodedInputStream::from_bytes(raw);
    let mut out = Vec::with_capacity(raw.len());
    {
        let mut os = CodedOutputStream::vec(&mut out);
        while let Some(tag) = is.read_raw_tag_or_eof().map_err(|e| e.to_string())? {
            let field_number = tag >> 3;
            let wire_type = tag & 0x07;
            if field_number == 0 {
                return Err("invalid protobuf field number 0".to_string());
            }
            match wire_type {
                WIRE_TYPE_VARINT => {
                    let val = is.read_raw_varint64().map_err(|e| e.to_string())?;
                    os.write_raw_varint32(tag).map_err(|e| e.to_string())?;
                    os.write_raw_varint64(val).map_err(|e| e.to_string())?;
                }
                WIRE_TYPE_FIXED64 => {
                    let val = is.read_raw_little_endian64().map_err(|e| e.to_string())?;
                    os.write_raw_varint32(tag).map_err(|e| e.to_string())?;
                    os.write_raw_little_endian64(val)
                        .map_err(|e| e.to_string())?;
                }
                WIRE_TYPE_LENGTH_DELIMITED => {
                    let bytes = is.read_bytes().map_err(|e| e.to_string())?;
                    os.write_raw_varint32(tag).map_err(|e| e.to_string())?;
                    match schema.get(field_number) {
                        Some(FieldSpec::String) => match std::str::from_utf8(&bytes) {
                            Ok(_) => {
                                os.write_bytes_no_tag(&bytes).map_err(|e| e.to_string())?;
                            }
                            Err(_) => {
                                stats.repaired_fields += 1;
                                let repaired = String::from_utf8_lossy(&bytes);
                                os.write_bytes_no_tag(repaired.as_bytes())
                                    .map_err(|e| e.to_string())?;
                            }
                        },
                        Some(FieldSpec::Message(child_schema)) => {
                            let child =
                                sanitize_message_bytes(&bytes, child_schema, depth + 1, stats)?;
                            os.write_bytes_no_tag(&child).map_err(|e| e.to_string())?;
                        }
                        None => {
                            os.write_bytes_no_tag(&bytes).map_err(|e| e.to_string())?;
                        }
                    }
                }
                WIRE_TYPE_FIXED32 => {
                    let val = is.read_raw_little_endian32().map_err(|e| e.to_string())?;
                    os.write_raw_varint32(tag).map_err(|e| e.to_string())?;
                    os.write_raw_little_endian32(val)
                        .map_err(|e| e.to_string())?;
                }
                other => {
                    return Err(format!("unsupported protobuf wire type {other}"));
                }
            }
        }
        os.flush().map_err(|e| e.to_string())?;
    }
    Ok(out)
}
