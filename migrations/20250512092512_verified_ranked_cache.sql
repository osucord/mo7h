ALTER TABLE verified_users
ADD COLUMN map_status SMALLINT;

ALTER TABLE verified_users
ADD COLUMN verified_roles BIGINT[];
