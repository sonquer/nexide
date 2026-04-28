/** @type {import('next').NextConfig} */
const nextConfig = {
  output: "standalone",
  outputFileTracingIncludes: {
    "/**": [
      "./prisma/dev.db",
      "./prisma/schema.prisma",
      "./node_modules/.prisma/client/**/*",
      "./node_modules/@prisma/client/**/*"
    ]
  }
};

export default nextConfig;
