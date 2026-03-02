/** @type {import('tailwindcss').Config} */
export default {
  darkMode: 'media',
  content: [
    "./index.html",
    "./src/**/*.{vue,js,ts,jsx,tsx}",
  ],
  theme: {
	  extend: {
		  colors: {
			  background: 'hsl(var(--background))',
			  foreground: 'hsl(var(--foreground))',
			  border: 'hsl(var(--border))',
			  primary: 'hsl(var(--primary))',
			  'primary-foreground': 'hsl(var(--primary-foreground))',
		  }
	  }
  },
  plugins: [require("tailwindcss-animate")],
}