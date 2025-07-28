CREATE TABLE private_vcs (
    channel_id INT PRIMARY KEY REFERENCES channels(id),
    owner_id INT NOT NULL REFERENCES users(id),
    message_id BIGINT REFERENCES messages(id),
    allowlist_roles BIGINT[] NOT NULL
);

-- unused as of migration as Phil is against this feature.
CREATE TABLE private_vc_trusted_users (
    channel_id INT REFERENCES private_vcs(channel_id) ON DELETE CASCADE,
    user_id INT REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (channel_id, user_id)
);

CREATE TABLE private_vc_allowlist_users (
    channel_id INT REFERENCES private_vcs(channel_id) ON DELETE CASCADE,
    user_id INT REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (channel_id, user_id)
);

CREATE TABLE private_vc_denylist_users (
    channel_id INT REFERENCES private_vcs(channel_id) ON DELETE CASCADE,
    user_id INT REFERENCES users(id) ON DELETE CASCADE,
    PRIMARY KEY (channel_id, user_id)
);
