pub mod client;
pub mod did_resolve;
pub mod plc;
pub mod repo;
pub mod service;
pub mod service_auth;

pub use client::{
    create_atp_post, create_atp_session, fetch_atp_history, fetch_bsky_followers,
    fetch_bsky_profile, fetch_single_bsky_post, search_appview_posts, upsert_bsky_post, AtpSession,
    BskyFollowerProfile, BskyPinnedPostRef, BskyPost, BskyProfile,
};
pub use did_resolve::{resolve_atproto_verification_key, DidResolveError};
pub use plc::{
    p256_to_did_key, plc_directory_base_url, prepare_plc_genesis, signing_key_from_pem,
    submit_plc_genesis, PlcError, PlcGenesis,
};
pub use repo::{
    build_commit_frame, build_error_frame, build_identity_frame, build_mst, cid_from_dagcbor,
    cid_from_sha256_hex, cid_from_str, cid_to_string, create_commit, encode_bsky_actor_profile,
    encode_bsky_feed_like, encode_bsky_feed_post, encode_bsky_feed_repost,
    encode_bsky_graph_follow, encode_car, generate_tid, BskyEmbed, BskyFacet, BskyFacetFeature,
    BskyFacetIndex, BskyFacetLink, BskyFacetMention, BskyImage, BskyPostReply, BskyRefRecord, Cid,
    CommitEvtOp, RepoError,
};
pub use service::{AtpCommitError, AtpCommitEvent, AtpCommitService};
pub use service_auth::{sign_service_auth_jwt, ServiceAuthError};
