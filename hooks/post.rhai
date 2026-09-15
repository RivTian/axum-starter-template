// Only rename/delete files in cargo-generate's own output directory.
file::rename("Makefile.project", "Makefile");
file::rename("README.project.md", "README.md");
file::delete("hooks");
