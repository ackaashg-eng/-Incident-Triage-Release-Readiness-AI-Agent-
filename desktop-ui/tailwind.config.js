/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  theme: {
    extend: {
      colors: {
        background: '#090d16',
        card: '#111726',
        'card-border': '#1e293b',
        accent: '#38bdf8',
        'accent-dim': '#0284c7',
        subtle: '#94a3b8',
      },
      fontFamily: {
        mono: ['Fira Code', 'Consolas', 'Courier New', 'monospace'],
      }
    },
  },
  plugins: [],
}
