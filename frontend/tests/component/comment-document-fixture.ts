/** A typed document for comment UI tests, independent of the full demo world. */
import type { Document } from '@/api/documents';

export const commentDocument: Document = {
  "id": "00000000-0000-4000-8007-000000000001",
  "title": "Rollout plan",
  "description": "",
  "description_source": "auto",
  "description_line": "Ship the redirect fix behind the flag, then:",
  "node": {
    "id": "00000000-0000-4000-8003-000000000003",
    "kind": "topic",
    "name": "Fix login",
    "redacted": false
  },
  "owner": {
    "id": "00000000-0000-4000-8001-000000000001",
    "kind": "human",
    "username": "ada",
    "display_name": "Ada Lovelace",
    "avatar": {
      "text": "A",
      "color": 1
    }
  },
  "owners": 1,
  "created_by": {
    "id": "00000000-0000-4000-8001-000000000001",
    "kind": "human",
    "username": "ada",
    "display_name": "Ada Lovelace",
    "avatar": {
      "text": "A",
      "color": 1
    }
  },
  "page": "follows",
  "seq": 1,
  "block_count": 25,
  "bytes": 1392,
  "files_bytes": 0,
  "version": 0,
  "open_discussions": 0,
  "updated_at": "2026-10-01T09:20:00.000Z",
  "updated_by": {
    "id": "00000000-0000-4000-8001-000000000001",
    "kind": "human",
    "username": "ada",
    "display_name": "Ada Lovelace",
    "avatar": {
      "text": "A",
      "color": 1
    }
  },
  "discussion_level": "involved",
  "seen_seq": null,
  "archived": false,
  "deleted": false,
  "path": [
    {
      "id": "00000000-0000-4000-8002-000000000064",
      "kind": "org",
      "name": "Acme",
      "redacted": false
    },
    {
      "id": "00000000-0000-4000-8003-000000000001",
      "kind": "org",
      "name": "Research",
      "redacted": false
    },
    {
      "id": "00000000-0000-4000-8003-000000000002",
      "kind": "project",
      "name": "Web",
      "redacted": false
    },
    {
      "id": "00000000-0000-4000-8003-000000000003",
      "kind": "topic",
      "name": "Fix login",
      "redacted": false
    }
  ],
  "location": {
    "parent": {
      "kind": "node_docs",
      "id": "00000000-0000-4000-8003-000000000003"
    },
    "basename": "Rollout plan.md",
    "ancestors": []
  },
  "files": [],
  "limits": {
    "bytes": 2097152,
    "blocks": 5000,
    "file_bytes": 8589934592,
    "files_bytes": 1073741824
  },
  "created_at": "2026-10-30T02:42:28.534Z",
  "access": {
    "permissions": [
      "access.manage",
      "access.own",
      "doc.comment",
      "doc.edit",
      "doc.export",
      "doc.manage",
      "doc.view"
    ],
    "administers": true,
    "owns": true,
    "participating": true,
    "admin_only": [],
    "admin_access": null
  }
};
