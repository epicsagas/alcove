use std::path::Path;

use anyhow::Result;

/// Helper to extract text from XML tags (e.g., w:t for Word, a:t for PPT)
#[cfg(feature = "alcove-full")]
pub(crate) fn extract_xml_text(content: &str, tag_name: &[u8]) -> Result<String> {
    use quick_xml::reader::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(content);
    let mut text = String::new();
    let mut in_tag = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().as_ref() == tag_name => {
                in_tag = true;
            }
            Ok(Event::End(ref e)) if e.name().as_ref() == tag_name => {
                in_tag = false;
            }
            Ok(Event::Text(e)) if in_tag => {
                if let Ok(s) = std::str::from_utf8(&e.into_inner()) {
                    text.push_str(
                        &quick_xml::escape::unescape(s).unwrap_or(std::borrow::Cow::Borrowed(s))
                    );
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(anyhow::anyhow!("XML parse error: {}", e)),
            _ => {}
        }
    }
    Ok(text)
}

/// Read file content, extracting text from PDF/DOCX if needed.
pub(crate) fn read_file_content(path: &Path) -> Result<String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();

    match ext.as_str() {
        #[cfg(all(unix, feature = "alcove-full"))]
        "pdf" => {
            use std::os::unix::io::AsRawFd;

            // pdf_extract prints unicode fallback noise to both stdout and stderr — suppress both.
            // FdGuard restores original fds on drop, protecting against panics.
            struct FdGuard {
                saved_stdout: libc::c_int,
                saved_stderr: libc::c_int,
            }
            impl Drop for FdGuard {
                fn drop(&mut self) {
                    unsafe {
                        if self.saved_stdout >= 0 {
                            libc::dup2(self.saved_stdout, libc::STDOUT_FILENO);
                            libc::close(self.saved_stdout);
                        }
                        if self.saved_stderr >= 0 {
                            libc::dup2(self.saved_stderr, libc::STDERR_FILENO);
                            libc::close(self.saved_stderr);
                        }
                    }
                }
            }
            let devnull = std::fs::File::open("/dev/null")
                .map_err(|e| anyhow::anyhow!("Failed to open /dev/null: {}", e))?;
            let devnull_fd = devnull.as_raw_fd();
            let saved_stdout = unsafe { libc::dup(libc::STDOUT_FILENO) };
            if saved_stdout < 0 {
                return Err(anyhow::anyhow!("dup(STDOUT_FILENO) failed"));
            }
            let saved_stderr = unsafe { libc::dup(libc::STDERR_FILENO) };
            if saved_stderr < 0 {
                unsafe { libc::close(saved_stdout); }
                return Err(anyhow::anyhow!("dup(STDERR_FILENO) failed"));
            }
            let _guard = FdGuard { saved_stdout, saved_stderr };
            unsafe {
                libc::dup2(devnull_fd, libc::STDOUT_FILENO);
                libc::dup2(devnull_fd, libc::STDERR_FILENO);
            }
            let result = pdf_extract::extract_text(path)
                .map_err(|e| anyhow::anyhow!("Failed to extract PDF: {}", e));
            // _guard drops here, restoring stdout/stderr automatically
            // Fallback to pdftotext if pdf_extract failed or returned empty content.
            // Uses spawn + try_wait with a 30-second deadline to prevent DoS via
            // a malformed PDF that makes pdftotext loop indefinitely.
            match result {
                Ok(text) if !text.trim().is_empty() => Ok(text),
                _ => {
                    use std::time::{Duration, Instant};
                    let pdftotext_bin = ["/usr/bin/pdftotext", "/usr/local/bin/pdftotext", "/opt/homebrew/bin/pdftotext"]
                        .iter()
                        .find(|p| std::path::Path::new(p).exists())
                        .copied()
                        .unwrap_or("pdftotext");
                    let mut child = std::process::Command::new(pdftotext_bin)
                        .args([path.as_os_str(), std::ffi::OsStr::new("-")])
                        .stdout(std::process::Stdio::piped())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .map_err(|e| anyhow::anyhow!("pdftotext not available: {}", e))?;
                    let deadline = Instant::now() + Duration::from_secs(30);
                    let status = loop {
                        match child.try_wait() {
                            Ok(Some(s)) => break Ok(s),
                            Ok(None) => {
                                if Instant::now() > deadline {
                                    let _ = child.kill();
                                    break Err(anyhow::anyhow!("pdftotext timed out"));
                                }
                                std::thread::sleep(Duration::from_millis(100));
                            }
                            Err(e) => break Err(anyhow::anyhow!("pdftotext wait error: {}", e)),
                        }
                    };
                    let status = status?;
                    if status.success() {
                        let mut stdout = child.stdout.take().unwrap_or_else(|| {
                            unreachable!("stdout pipe must be present after spawn")
                        });
                        let mut buf = Vec::new();
                        use std::io::Read;
                        stdout.read_to_end(&mut buf)
                            .map_err(|e| anyhow::anyhow!("pdftotext read error: {}", e))?;
                        String::from_utf8(buf)
                            .map_err(|e| anyhow::anyhow!("pdftotext output not UTF-8: {}", e))
                    } else {
                        Err(anyhow::anyhow!("pdftotext failed"))
                    }
                }
            }
        }
        #[cfg(feature = "alcove-full")]
        "docx" | "pptx" => {
            use std::io::Read;
            let file = std::fs::File::open(path)?;
            let mut archive = zip::ZipArchive::new(file)
                .map_err(|e| anyhow::anyhow!("Failed to open {} (ZIP): {}", ext, e))?;

            let mut text = String::new();

            if ext == "docx" {
                let mut doc_xml = archive.by_name("word/document.xml")
                    .map_err(|e| anyhow::anyhow!("Failed to find word/document.xml in DOCX: {}", e))?;
                let mut content = String::new();
                doc_xml.read_to_string(&mut content)?;
                text = extract_xml_text(&content, b"w:t")?;
            } else {
                // PPTX: iterate through slides
                let mut slide_names: Vec<String> = archive.file_names()
                    .filter(|n| n.starts_with("ppt/slides/slide") && n.ends_with(".xml"))
                    .map(|n| n.to_string())
                    .collect();
                slide_names.sort_by_key(|n| {
                    n.trim_start_matches("ppt/slides/slide")
                     .trim_end_matches(".xml")
                     .parse::<u32>().unwrap_or(0)
                });

                for name in slide_names {
                    let mut slide_xml = archive.by_name(&name)?;
                    let mut content = String::new();
                    slide_xml.read_to_string(&mut content)?;
                    let slide_text = extract_xml_text(&content, b"a:t")?;
                    if !slide_text.is_empty() {
                        text.push_str(&format!("\n--- Slide {} ---\n", name));
                        text.push_str(&slide_text);
                    }
                }
            }
            Ok(text)
        }
        #[cfg(feature = "alcove-full")]
        "xlsx" | "csv" => {
            use calamine::{Reader, open_workbook_auto};
            let mut workbook = open_workbook_auto(path)
                .map_err(|e| anyhow::anyhow!("Failed to open Excel/CSV: {}", e))?;

            let mut text = String::new();
            // Process all sheets
            for sheet_name in workbook.sheet_names().to_owned() {
                if let Ok(range) = workbook.worksheet_range(&sheet_name) {
                    text.push_str(&format!("\n--- Sheet: {} ---\n", sheet_name));
                    for row in range.rows() {
                        let row_text: Vec<String> = row.iter().map(|c| match c {
                            calamine::Data::Empty => "".to_string(),
                            calamine::Data::String(s) => s.clone(),
                            calamine::Data::Float(f) => f.to_string(),
                            calamine::Data::Int(i) => i.to_string(),
                            calamine::Data::Bool(b) => b.to_string(),
                            _ => "".to_string(),
                        }).collect();
                        text.push_str(&row_text.join("\t"));
                        text.push('\n');
                    }
                }
            }
            Ok(text)
        }
        _ => std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read file {}: {}", path.display(), e)),
    }
}
