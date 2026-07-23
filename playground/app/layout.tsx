import type { Metadata } from "next";
import { Geist, Geist_Mono } from "next/font/google";
import "./globals.css";

const geistSans = Geist({ variable: "--font-sans", subsets: ["latin"] });
const geistMono = Geist_Mono({ variable: "--font-mono", subsets: ["latin"] });

export const metadata: Metadata = {
  metadataBase: new URL("https://lulang.lulzx.space"),
  title: {
    default: "lulang — a language for numerical computing",
    template: "%s · lulang",
  },
  description:
    "A small language for numerical computing with native code generation and C and Python interfaces.",
  openGraph: {
    title: "lulang — a language for numerical computing",
    description: "Value semantics, native code generation, and C and Python interfaces.",
    url: "https://lulang.lulzx.space",
    siteName: "lulang",
    images: [{ url: "/og.png", width: 1200, height: 630 }],
    type: "website",
  },
  twitter: {
    card: "summary_large_image",
    title: "lulang — a language for numerical computing",
    description: "Value semantics, native code generation, and C and Python interfaces.",
    images: ["/og.png"],
  },
  icons: { icon: "/og.png" },
};

export default function RootLayout({ children }: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body className={`${geistSans.variable} ${geistMono.variable}`}>{children}</body>
    </html>
  );
}
