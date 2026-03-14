use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    let documents = langextract_host::execute_pipeline(
        "The separation agreement for CoBro was signed by Jens and Ryan.",
        "Identify all founders and signatories.",
        vec![],
    )
    .await?;

    println!(
        "Received {} document(s) from compliance shim.",
        documents.len()
    );
    for doc in &documents {
        println!(
            "Doc ID: {}, Extractions: {}",
            doc.document_id,
            doc.extractions.len()
        );
        for ext in &doc.extractions {
            println!(
                "  [{}] {}: '{}'",
                ext.alignment_status, ext.extraction_class, ext.extraction_text
            );
        }
    }

    Ok(())
}
