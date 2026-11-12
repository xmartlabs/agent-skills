/** "../../specimens/forms/Field.tsx" -> "forms/Field". */
export function specimenName(file) {
  return file.replace(/^.*?\/specimens\//, "").replace(/\.tsx$/, "");
}
