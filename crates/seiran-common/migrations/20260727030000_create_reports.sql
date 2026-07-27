CREATE TYPE report_subject_type AS ENUM ('actor', 'post');
CREATE TYPE report_destination AS ENUM ('local', 'remote');
CREATE TYPE report_status AS ENUM ('open', 'closed');

CREATE TABLE reports (
    id BIGINT PRIMARY KEY,
    reporter_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    subject_type report_subject_type NOT NULL,
    subject_actor_id BIGINT NOT NULL REFERENCES actors(id) ON DELETE CASCADE,
    subject_post_id BIGINT REFERENCES posts(id) ON DELETE SET NULL,
    reason_type VARCHAR(64) NOT NULL,
    reason_text TEXT NOT NULL DEFAULT '',
    destination report_destination NOT NULL,
    remote_host VARCHAR(255),
    status report_status NOT NULL DEFAULT 'open',
    forwarded_at TIMESTAMP WITH TIME ZONE,
    closed_at TIMESTAMP WITH TIME ZONE,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
    CONSTRAINT reports_subject_consistent CHECK (
        (subject_type = 'actor' AND subject_post_id IS NULL)
        OR (subject_type = 'post' AND subject_post_id IS NOT NULL)
    ),
    CONSTRAINT reports_destination_consistent CHECK (
        destination = 'local' OR remote_host IS NOT NULL
    ),
    CONSTRAINT reports_reason_text_limits CHECK (
        char_length(reason_text) <= 300 AND octet_length(reason_text) <= 1000
    )
);

CREATE INDEX idx_reports_status_created
    ON reports(status, created_at DESC);
CREATE INDEX idx_reports_subject_actor
    ON reports(subject_actor_id, created_at DESC);
CREATE INDEX idx_reports_subject_post
    ON reports(subject_post_id, created_at DESC)
    WHERE subject_post_id IS NOT NULL;

CREATE TABLE report_comments (
    id BIGINT PRIMARY KEY,
    report_id BIGINT NOT NULL REFERENCES reports(id) ON DELETE CASCADE,
    author_user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    body TEXT NOT NULL CHECK (char_length(body) BETWEEN 1 AND 2000),
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_report_comments_report
    ON report_comments(report_id, created_at);
