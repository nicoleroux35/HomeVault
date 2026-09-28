-- RAG (Retrieval-Augmented Generation) knowledge base
-- Stores document chunks with embeddings for semantic search

CREATE TABLE IF NOT EXISTS rag_documents (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,          -- 'catalog', 'i18n', 'features', 'system', 'settings'
    source_id TEXT NOT NULL,       -- e.g. app id, i18n section, feature name
    title TEXT NOT NULL,
    content TEXT NOT NULL,         -- full original text
    language TEXT DEFAULT 'en',
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS rag_chunks (
    id TEXT PRIMARY KEY,
    document_id TEXT NOT NULL REFERENCES rag_documents(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    content TEXT NOT NULL,
    embedding BLOB,               -- f32 vector serialized as bytes
    token_count INTEGER DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_rag_chunks_doc ON rag_chunks(document_id);
CREATE INDEX IF NOT EXISTS idx_rag_docs_source ON rag_documents(source, source_id);

CREATE TABLE IF NOT EXISTS rag_index_status (
    id INTEGER PRIMARY KEY DEFAULT 1,
    last_indexed_at TEXT,
    total_documents INTEGER DEFAULT 0,
    total_chunks INTEGER DEFAULT 0,
    embedding_model TEXT,
    status TEXT DEFAULT 'idle'    -- 'idle', 'indexing', 'error'
);
