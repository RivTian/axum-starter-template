// post hook：在模板展开之后、输出落到目标目录之前运行。
// 1. 模板仓库自己的 Makefile（gen / lock / 回写）与 README（怎么用模板）不进生成结果
//    （见 cargo-generate.toml 的 ignore）；生成的项目拿到的是 Makefile.project 与
//    README.project.md，这里改回正名。
// 2. hooks/ 自删：它不能放进 ignore（ignore 先于 post hook 生效，脚本会先于自己被删）。
file::rename("Makefile.project", "Makefile");
file::rename("README.project.md", "README.md");
file::delete("hooks");
