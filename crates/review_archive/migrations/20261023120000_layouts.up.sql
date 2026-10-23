-- the board's dock layout fresh browsers start from, per dock and band; `layout` is the dock's own JSON
CREATE TABLE layouts (
    dock         TEXT NOT NULL,
    band         TEXT NOT NULL,
    layout       TEXT NOT NULL,
    published_at TEXT NOT NULL,
    PRIMARY KEY (dock, band)
);
