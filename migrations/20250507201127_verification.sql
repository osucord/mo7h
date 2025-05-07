CREATE TABLE verified_users(
    user_id BIGINT NOT NULL,
    osu_id INT NOT NULL,
    last_updated BIGINT NOT NULL,
    is_active BOOLEAN NOT NULL,
    gamemode SMALLINT NOT NULL,
    PRIMARY KEY (user_id),
    FOREIGN KEY (user_id) REFERENCES users(user_id)
)
