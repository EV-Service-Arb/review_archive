-- the conductor's walk prices as of its last snapshot, by `sa_core::prices::Item`; none until it
-- sends one, and a step without a row is not walked
CREATE TABLE mirror_prices (
	item TEXT PRIMARY KEY,
	price INTEGER NOT NULL CHECK (price >= 0)
);
