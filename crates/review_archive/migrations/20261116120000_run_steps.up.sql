-- an open run's walk steps so far, `{"<sa_core::prices::Item>": units}`, for the next start to count a walk the process died in
ALTER TABLE runs ADD COLUMN steps TEXT;
