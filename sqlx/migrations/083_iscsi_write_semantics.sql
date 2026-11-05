-- iSCSI backstore write semantics (tasklist P0-12).
--
-- LIO advertises a write cache and honours FUA/UNMAP through per-backstore
-- attributes. HomeVault used the kernel defaults without saying so; these
-- columns make the policy explicit and survive an apply/restart.
ALTER TABLE iscsi_backstores ADD COLUMN write_policy TEXT NOT NULL DEFAULT 'buffered';
ALTER TABLE iscsi_backstores ADD COLUMN emulate_fua INTEGER NOT NULL DEFAULT 1;
ALTER TABLE iscsi_backstores ADD COLUMN unmap INTEGER NOT NULL DEFAULT 1;
