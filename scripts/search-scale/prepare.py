#!/usr/bin/env python3
"""Prepare the native isolated search fixture with psql and the canonical migration and corpus SQL."""

import json
import os
from pathlib import Path
import subprocess


# Separate commits keep deferred row-trigger memory bounded, just as the compiled preparer does.
DOCUMENT_BATCH = 10_000
DIMENSIONS = 256


def finish_index(database_url, documents, output):
    """Finish only a fully populated native fixture whose generation index has not completed."""
    command = ["psql", database_url, "-X", "-Atq", "-v", "ON_ERROR_STOP=1"]
    inspection = subprocess.run([*command, "-c", """SELECT jsonb_build_object(
      'documents',(SELECT count(*) FROM documents),'blocks',(SELECT count(*) FROM document_blocks),
      'messages',(SELECT count(*) FROM chat_messages),'items',(SELECT count(*) FROM items),
      'chunks',(SELECT count(*) FROM search_chunks WHERE generation_id=search_probe_id('generation')),
      'vectors',(SELECT count(*) FROM search_vectors WHERE generation_id=search_probe_id('generation')),
      'generations',(SELECT count(*) FROM search_generations),
      'dimensions',(SELECT dimensions FROM search_generations WHERE id=search_probe_id('generation')),
      'prototypes',(SELECT count(*) FROM corpus_vector_prototypes),
      'index_exists',to_regclass('search_vectors_'||replace(search_probe_id('generation')::text,'-','')) IS NOT NULL
    )"""], check=True, text=True, capture_output=True)
    found = json.loads(inspection.stdout)
    expected = {"documents": documents, "blocks": documents, "messages": documents // 5,
                "items": documents // 5, "chunks": documents * 7 // 5, "vectors": documents * 7 // 5,
                "generations": 1, "dimensions": DIMENSIONS, "prototypes": 8192, "index_exists": False}
    if found != expected:
        (output / "output.log").write_text("Refused incomplete or already indexed fixture:\n" + json.dumps(found, indent=2) + "\n")
        return 2
    # The measured graph exceeded 1 GiB after 875,065 vectors: 1.4 million need about 1.6 GiB.
    # Two GiB gives headroom while leaving the four-GiB fixture room for its shared buffers and backend.
    env = os.environ.copy()
    env["PGOPTIONS"] = "-c statement_timeout=0 -c jit=off -c maintenance_work_mem=2GB -c max_parallel_maintenance_workers=0"
    statement = f"""SELECT 'search_vectors_'||replace(search_probe_id('generation')::text,'-','') AS index_name,
 search_probe_id('generation')::text AS generation \\gset
CREATE INDEX :index_name ON search_vectors
 USING hnsw ((subvector(embedding,1,{DIMENSIONS})::halfvec({DIMENSIONS})) halfvec_cosine_ops)
 WHERE generation_id=:'generation';
ANALYZE search_vectors;
"""
    with (output / "output.log").open("x") as log:
        result = subprocess.run(command, input=statement, text=True, env=env,
                                stdout=log, stderr=subprocess.STDOUT, check=False)
    if result.returncode == 0:
        report = [{"prepared": True, "profile": "not_applicable", "documents": documents,
                   "chat_messages": documents // 5, "thread_items": documents // 5,
                   "vectors": documents * 7 // 5, "storage": "native_tables_and_generation_hnsw",
                   "preparation_engine": "psql_finish_index", "maintenance_work_mem": "2GB"}]
        (output / "measurements.json").write_text(json.dumps(report, indent=2) + "\n")
    return result.returncode


def prepare(database_url, documents, output):
    """Refuse existing data, keep constraints enabled, and build the ordinary generation index."""
    root = Path(__file__).resolve().parents[2]
    corpus = (Path(__file__).with_name("corpus.sql")).read_text()
    before, rest = corpus.split("-- @documents\n", 1)
    document_sql, after = rest.split("-- @content\n", 1)
    env = os.environ.copy()
    env["PGOPTIONS"] = "-c statement_timeout=0 -c jit=off"
    with (output / "output.log").open("x") as log:
        process = subprocess.Popen(
            ["psql", database_url, "-X", "-q", "-v", "ON_ERROR_STOP=1"],
            env=env, stdin=subprocess.PIPE, stdout=log, stderr=subprocess.STDOUT, text=True,
        )
        try:
            assert process.stdin is not None
            stream = process.stdin
            stream.write("""DO $$ BEGIN
IF EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace
 WHERE n.nspname='public' AND c.relkind IN ('r','p','v','m','f')) THEN
 RAISE EXCEPTION 'never migrate or replace an existing database for a benchmark';
END IF; END $$;
""")
            for migration in sorted((root / "migrations").glob("[0-9]*.sql")):
                stream.write("BEGIN;\n" + migration.read_text() + "\nCOMMIT;\n")
            # Two GiB fits the measured 1.4-million-vector graph in the four-GiB fixture.
            # This preparation memory is separate from measured query limits.
            stream.write("SET maintenance_work_mem='2GB';SET max_parallel_maintenance_workers=0;\n")
            stream.write(f"SET orch_probe.documents='{documents}';\n" + before)
            for first in range(1, documents + 1, DOCUMENT_BATCH):
                last = min(first + DOCUMENT_BATCH - 1, documents)
                stream.write(document_sql.replace("$1", str(first)).replace("$2", str(last)))
                stream.write(f"\n\\echo Prepared {last} documents\n")
            for phase, sql in enumerate(after.split("-- @phase\n")):
                stream.write(sql)
                stream.write(f"\n\\echo Prepared content phase {phase}\n")
            # This is the index expression used by the indexer's make_index, including its generation predicate.
            stream.write(f"""
SELECT 'search_vectors_'||replace(search_probe_id('generation')::text,'-','') AS index_name,
 search_probe_id('generation')::text AS generation \\gset
CREATE INDEX :index_name ON search_vectors
 USING hnsw ((subvector(embedding,1,{DIMENSIONS})::halfvec({DIMENSIONS})) halfvec_cosine_ops)
 WHERE generation_id=:'generation';
ANALYZE search_vectors;
\\echo Prepared generation index
""")
            stream.close()
            result = process.wait()
        except (BrokenPipeError, OSError):
            # psql's ON_ERROR_STOP preserves the first database refusal in output.log.
            if process.stdin is not None and not process.stdin.closed:
                process.stdin.close()
            result = process.wait()
            if result == 0:
                raise
    if result:
        return result
    report = [{"prepared": True, "profile": "not_applicable", "documents": documents,
               "chat_messages": documents // 5, "thread_items": documents // 5,
               "vectors": documents * 7 // 5, "storage": "native_tables_and_generation_hnsw",
               "preparation_engine": "psql"}]
    (output / "measurements.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0
