CREATE TABLE transcendent_roles (
    id SMALLSERIAL PRIMARY KEY,
    -- not normalized because Ruben will do what he wants when he gets this.
    user_id BIGINT NOT NULL,
    role_id BIGINT NOT NULL
);
