CREATE TABLE IF NOT EXISTS peer (
    guid blob primary key not null,
    id varchar(100) not null,
    uuid blob not null,
    pk blob not null,
    created_at datetime not null default(current_timestamp),
    user blob,
    status tinyint,
    note varchar(300),
    info text not null
) without rowid;

CREATE UNIQUE INDEX IF NOT EXISTS index_peer_id ON peer (id);
CREATE INDEX IF NOT EXISTS index_peer_user ON peer (user);
CREATE INDEX IF NOT EXISTS index_peer_created_at ON peer (created_at);
CREATE INDEX IF NOT EXISTS index_peer_status ON peer (status);
