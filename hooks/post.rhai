// Only rename/delete files in cargo-generate's own output directory.
file::rename("Makefile.project", "Makefile");
file::rename("README.project.md", "README.md");
// A non-workflow suffix prevents the project workflow running in this template.
// Moving the directory also preserves GitHub expressions without Liquid parsing.
file::rename("project-ci", ".github");
file::delete("hooks");
