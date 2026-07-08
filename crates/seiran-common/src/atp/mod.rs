pub mod did;
pub mod client;
pub mod plc;
pub mod repo;
pub mod service;

pub use did::{resolve_did, DidDocument, DidService};
pub use client::{
    fetch_atp_history, create_atp_session, create_atp_post,
    BskyPost, AtpSession,
};
pub use plc::{prepare_plc_genesis, submit_plc_genesis, PlcGenesis, p256_to_did_key, signing_key_from_pem, PlcError};
pub use repo::{
    Cid,
    generate_tid, cid_from_dagcbor, cid_from_str, cid_to_string, cid_from_sha256_hex,
    build_mst, create_commit, encode_car, encode_bsky_feed_post, encode_bsky_actor_profile,
    encode_bsky_feed_repost, encode_bsky_graph_follow,
    build_commit_frame, build_identity_frame, build_error_frame, CommitEvtOp, RepoError,
    BskyFacet, BskyFacetIndex, BskyFacetMention, BskyImage, BskyEmbed,
    BskyRefRecord, BskyPostReply,
};
pub use service::{AtpCommitService, AtpCommitError, AtpCommitEvent};
