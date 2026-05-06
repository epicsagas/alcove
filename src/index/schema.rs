use anyhow::Result;
use tantivy::schema::{Field, IndexRecordOption, Schema, STRING, STORED, TextFieldIndexing, TextOptions, INDEXED};
use tantivy::tokenizer::{LowerCaser, NgramTokenizer, TextAnalyzer};
use tantivy::Index;

pub(crate) const NGRAM_TOKENIZER: &str = "cjk_ngram";

/// Current schema version. Increment when fields change to force a full rebuild.
pub(crate) const SCHEMA_VERSION: u32 = 2;

/// All Tantivy schema fields in one place.
pub struct IndexSchema {
    pub schema:     Schema,
    pub project:    Field,
    pub file:       Field,
    pub filename:   Field,
    pub title:      Field,
    pub chunk_id:   Field,
    pub body:       Field,
    pub line_start: Field,
}

impl IndexSchema {
    pub fn build() -> Self {
        let mut builder = Schema::builder();
        let project = builder.add_text_field("project", STRING | STORED);
        let file = builder.add_text_field("file", STRING | STORED); // exact match for deletes/filtering

        let ngram_indexing = TextFieldIndexing::default()
            .set_tokenizer(NGRAM_TOKENIZER)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions);
        let ngram_options = TextOptions::default()
            .set_indexing_options(ngram_indexing)
            .set_stored();

        let filename = builder.add_text_field("filename", ngram_options.clone()); // tokenized for search
        let title = builder.add_text_field("title", ngram_options); // tokenized for search

        let chunk_id = builder.add_u64_field("chunk_id", INDEXED | STORED);

        let body_indexing = TextFieldIndexing::default()
            .set_tokenizer(NGRAM_TOKENIZER)
            .set_index_option(IndexRecordOption::WithFreqsAndPositions);
        let body_options = TextOptions::default()
            .set_indexing_options(body_indexing)
            .set_stored();
        let body = builder.add_text_field("body", body_options);
        let line_start = builder.add_u64_field("line_start", STORED);

        Self {
            schema: builder.build(),
            project,
            file,
            filename,
            title,
            chunk_id,
            body,
            line_start,
        }
    }
}

pub(crate) fn register_ngram_tokenizer(index: &Index) -> Result<()> {
    let ngram = TextAnalyzer::builder(NgramTokenizer::new(2, 3, false).map_err(|e| {
        anyhow::anyhow!("Failed to create NgramTokenizer: {}", e)
    })?)
    .filter(LowerCaser)
    .build();
    index.tokenizers().register(NGRAM_TOKENIZER, ngram);
    Ok(())
}
