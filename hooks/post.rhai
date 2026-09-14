// Only rename/delete files in cargo-generate's own output directory.
file::rename("Makefile.project", "Makefile");
file::rename("README.project.md", "README.md");
// Outside .github/workflows, the project workflow cannot run in the template repository.
// Moving the directory also preserves GitHub expressions without Liquid parsing.
file::rename("project-ci", ".github");
file::delete("hooks");
