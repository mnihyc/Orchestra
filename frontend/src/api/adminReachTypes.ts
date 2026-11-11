/** Shared AR view names refer directly to the generated, joined Rust contract. */
export type {
  AdminToken, AdminSession, AdminTokenActivity, UploadAttachment, UploadInventoryItem,
  StorageArtifact, StoragePart, StorageKind, KeptRecord, StorageQuery, StorageReport,
  Snapshot as ReachSnapshot, Export as ReachExport, Import as ReachImport,
} from '@/generated/api-types';
export interface ReachPage<Row> { items: Row[]; next_cursor: string | null }
