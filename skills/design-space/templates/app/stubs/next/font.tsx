// Stand-in for next/font/google and next/font/local: nothing is downloaded. The family name is
// applied as a plain font-family, so a font the product loads in its own page falls back to the
// system font here unless the design space page loads it (the scaffold links the Google Fonts it finds).
export type FontResult = { className: string; style: { fontFamily: string }; variable: string };
type FontOptions = { variable?: string; [option: string]: unknown };

export const createFont = (family: string) => (options?: FontOptions): FontResult => {
  void options;
  return { className: "", style: { fontFamily: family }, variable: "" };
};

// Google families the product commonly imports. The scaffold appends the ones it finds in the
// product's layouts; add any other family the same way: export const Family_Name = createFont("Family Name").
export const Inter = createFont("Inter");
export const Roboto = createFont("Roboto");
export const Roboto_Mono = createFont("Roboto Mono");
export const Open_Sans = createFont("Open Sans");
export const Lato = createFont("Lato");
export const Montserrat = createFont("Montserrat");
export const Poppins = createFont("Poppins");
export const Nunito = createFont("Nunito");
export const Nunito_Sans = createFont("Nunito Sans");
export const DM_Sans = createFont("DM Sans");
export const Manrope = createFont("Manrope");
export const Work_Sans = createFont("Work Sans");
export const Source_Sans_3 = createFont("Source Sans 3");
export const Playfair_Display = createFont("Playfair Display");
export const Merriweather = createFont("Merriweather");
export const Geist = createFont("Geist");
export const Geist_Mono = createFont("Geist Mono");
export const JetBrains_Mono = createFont("JetBrains Mono");
export const Fira_Code = createFont("Fira Code");

// next/font/local: `localFont({ src: ... })`.
export default function localFont(options?: FontOptions): FontResult {
  void options;
  return { className: "", style: { fontFamily: "sans-serif" }, variable: "" };
}
