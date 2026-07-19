pub mod client;
pub mod did_resolve;
pub mod plc;
pub mod repo;
pub mod service;
pub mod service_auth;

pub use client::{
    fetch_atp_history, fetch_single_bsky_post, fetch_bsky_profile, search_appview_posts,
    create_atp_session, create_atp_post, upsert_bsky_post,
    BskyPost, BskyProfile, BskyPinnedPostRef, AtpSession,
};
pub use plc::{prepare_plc_genesis, submit_plc_genesis, plc_directory_base_url, PlcGenesis, p256_to_did_key, signing_key_from_pem, PlcError};
pub use repo::{
    Cid,
    generate_tid, cid_from_dagcbor, cid_from_str, cid_to_string, cid_from_sha256_hex,
    build_mst, create_commit, encode_car, encode_bsky_feed_post, encode_bsky_actor_profile,
    encode_bsky_feed_repost, encode_bsky_feed_like, encode_bsky_graph_follow,
    build_commit_frame, build_identity_frame, build_error_frame, CommitEvtOp, RepoError,
    BskyFacet, BskyFacetIndex, BskyFacetMention, BskyFacetLink, BskyFacetFeature, BskyImage, BskyEmbed,
    BskyRefRecord, BskyPostReply,
};
pub use service::{AtpCommitService, AtpCommitError, AtpCommitEvent};
pub use service_auth::{sign_service_auth_jwt, ServiceAuthError};
pub use did_resolve::{resolve_atproto_verification_key, DidResolveError};
