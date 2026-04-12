use anyhow::{Context, Result};
#[cfg(test)]
use lopdf::dictionary;
use lopdf::encryption::crypt_filters::{Aes128CryptFilter, CryptFilter};
use lopdf::{Document, EncryptionState, EncryptionVersion, Permissions};
use std::collections::BTreeMap;
use std::sync::Arc;

pub fn seal_pdf(bytes: &[u8], password: &str) -> Result<Vec<u8>> {
    let mut doc = Document::load_mem(bytes).context("parse payslip PDF")?;

    let crypt_filter: Arc<dyn CryptFilter> = Arc::new(Aes128CryptFilter);
    let version = EncryptionVersion::V4 {
        document: &doc,
        encrypt_metadata: true,
        crypt_filters: BTreeMap::from([(b"StdCF".to_vec(), crypt_filter)]),
        stream_filter: b"StdCF".to_vec(),
        string_filter: b"StdCF".to_vec(),
        owner_password: password,
        user_password: password,
        permissions: Permissions::all(),
    };
    let state = EncryptionState::try_from(version).context("build PDF encryption state")?;
    doc.encrypt(&state).context("encrypt payslip PDF")?;

    let mut out = Vec::new();
    doc.save_to(&mut out)
        .context("serialize encrypted payslip PDF")?;
    Ok(out)
}

pub fn unseal_pdf(bytes: &[u8], password: &str) -> Result<Vec<u8>> {
    let mut doc = Document::load_mem_with_password(bytes, password)
        .context("decrypt password-protected payslip PDF")?;
    let mut out = Vec::new();
    doc.save_to(&mut out)
        .context("serialize decrypted payslip PDF")?;
    Ok(out)
}

#[cfg(test)]
pub(crate) fn sample_pdf_bytes() -> Vec<u8> {
    use lopdf::{Object, Stream};

    let mut doc = Document::with_version("1.5");
    let id1 = vec![1_u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
    let id2 = vec![16_u8, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1];
    doc.trailer.set(
        "ID",
        Object::Array(vec![
            Object::String(id1, lopdf::StringFormat::Literal),
            Object::String(id2, lopdf::StringFormat::Literal),
        ]),
    );

    let pages_id = doc.new_object_id();
    let page_id = doc.new_object_id();
    let content_id = doc.new_object_id();

    let catalog_id = doc.add_object(dictionary! {
        "Type" => "Catalog",
        "Pages" => Object::Reference(pages_id),
    });
    doc.trailer.set("Root", Object::Reference(catalog_id));

    doc.objects.insert(
        pages_id,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => vec![Object::Reference(page_id)],
            "Count" => 1,
        }),
    );

    doc.objects.insert(
        page_id,
        Object::Dictionary(dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(pages_id),
            "MediaBox" => vec![
                Object::Integer(0),
                Object::Integer(0),
                Object::Integer(612),
                Object::Integer(792),
            ],
            "Contents" => Object::Reference(content_id),
        }),
    );

    let content = b"BT\n/F1 12 Tf\n100 700 Td\n(Protected Payslip) Tj\nET\n";
    doc.objects.insert(
        content_id,
        Object::Stream(Stream::new(dictionary! {}, content.to_vec())),
    );

    let mut out = Vec::new();
    doc.save_to(&mut out).unwrap();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_and_unseal_pdf_roundtrip() {
        let original = sample_pdf_bytes();
        let sealed = seal_pdf(&original, "s3cret").expect("seal sample PDF");

        assert_ne!(sealed, original);
        let sealed_text = String::from_utf8_lossy(&sealed);
        assert!(sealed_text.contains("/V 4"), "sealed PDF should use /V 4");

        let unsealed = unseal_pdf(&sealed, "s3cret").expect("unseal sample PDF");
        let doc = Document::load_mem(&unsealed).expect("parse decrypted PDF");
        assert_eq!(doc.get_pages().len(), 1);
    }

    #[test]
    fn unseal_pdf_with_wrong_password_fails() {
        let sealed = seal_pdf(&sample_pdf_bytes(), "s3cret").expect("seal sample PDF");
        let err = unseal_pdf(&sealed, "wrong-password").expect_err("wrong password should fail");
        assert!(
            err.to_string().contains("decrypt")
                || err.to_string().contains("password")
                || err.to_string().contains("authentication"),
            "unexpected error: {err}"
        );
    }
}
