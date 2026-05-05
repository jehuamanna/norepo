use loro::LoroDoc;

/// Best-effort markdown projection. Looks for a text container at path "body";
/// otherwise returns an empty string. Used by import flows that materialise
/// a Loro doc back to plain markdown for Local-flavour storage.
pub fn doc_to_markdown(doc: &LoroDoc) -> String {
    let text = doc.get_text("body");
    text.to_string()
}

/// Seed a fresh LoroDoc from a markdown body. Inserts the markdown into the
/// "body" text container at offset 0.
pub fn seed_doc_from_markdown(doc: &LoroDoc, body: &str) -> Result<(), String> {
    let text = doc.get_text("body");
    text.insert(0, body).map_err(|e| e.to_string())?;
    doc.commit();
    Ok(())
}
