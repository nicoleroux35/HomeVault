/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{svelte,js,ts,jsx,tsx}",
  ],
  darkMode: 'class',
  theme: {
    extend: {
      // Use --hv-* CSS variables as single source of truth
      colors: {
        // Primary colors
        primary: {
          DEFAULT: 'var(--hv-primary)',
          50: 'var(--hv-primary-50)',
          100: 'var(--hv-primary-100)',
          200: 'var(--hv-primary-200)',
          300: 'var(--hv-primary-300)',
          400: 'var(--hv-primary-400)',
          500: 'var(--hv-primary)',
          600: 'var(--hv-primary-600)',
          700: 'var(--hv-primary-700)',
          light: 'var(--hv-primary-light)',
        },
        // Semantic colors
        success: {
          DEFAULT: 'var(--hv-success)',
          light: 'var(--hv-success-light)',
        },
        warning: {
          DEFAULT: 'var(--hv-warning)',
          light: 'var(--hv-warning-light)',
        },
        danger: {
          DEFAULT: 'var(--hv-danger)',
        },
        error: {
          DEFAULT: 'var(--hv-error)',
          light: 'var(--hv-error-light)',
        },
        info: {
          DEFAULT: 'var(--hv-info)',
          light: 'var(--hv-info-light)',
        },
        // Neutrals
        neutral: {
          50: 'var(--hv-neutral-50)',
          100: 'var(--hv-neutral-100)',
          200: 'var(--hv-neutral-200)',
          300: 'var(--hv-neutral-300)',
          400: 'var(--hv-neutral-400)',
          500: 'var(--hv-neutral-500)',
          600: 'var(--hv-neutral-600)',
          700: 'var(--hv-neutral-700)',
          800: 'var(--hv-neutral-800)',
          900: 'var(--hv-neutral-900)',
        },
        // Contextual
        background: {
          DEFAULT: 'var(--hv-background)',
          alt: 'var(--hv-background-alt)',
        },
        text: {
          DEFAULT: 'var(--hv-text-primary)',
          secondary: 'var(--hv-text-secondary)',
          muted: 'var(--hv-text-tertiary)',
        },
        border: {
          DEFAULT: 'var(--hv-border)',
        },
      },
      fontFamily: {
        sans: 'var(--hv-font-family)',
        mono: 'var(--hv-font-mono)',
      },
      fontSize: {
        xs: 'var(--hv-font-size-xs)',
        sm: 'var(--hv-font-size-sm)',
        base: 'var(--hv-font-size-base)',
        lg: 'var(--hv-font-size-lg)',
        xl: 'var(--hv-font-size-xl)',
        '2xl': 'var(--hv-font-size-2xl)',
        '3xl': 'var(--hv-font-size-3xl)',
      },
      spacing: {
        '1': 'var(--hv-space-1)',
        '2': 'var(--hv-space-2)',
        '3': 'var(--hv-space-3)',
        '4': 'var(--hv-space-4)',
        '5': 'var(--hv-space-5)',
        '6': 'var(--hv-space-6)',
        '8': 'var(--hv-space-8)',
        '10': 'var(--hv-space-10)',
        '12': 'var(--hv-space-12)',
        '16': 'var(--hv-space-16)',
      },
      borderRadius: {
        sm: 'var(--hv-radius-sm)',
        DEFAULT: 'var(--hv-radius-default)',
        md: 'var(--hv-radius-md)',
        lg: 'var(--hv-radius-lg)',
        xl: 'var(--hv-radius-xl)',
        full: '9999px',
      },
      boxShadow: {
        sm: 'var(--hv-shadow-sm)',
        DEFAULT: 'var(--hv-shadow-md)',
        md: 'var(--hv-shadow-md)',
        lg: 'var(--hv-shadow-lg)',
        xl: 'var(--hv-shadow-xl)',
        '2xl': 'var(--hv-shadow-2xl)',
      },
      transitionDuration: {
        fast: 'var(--hv-transition-fast)',
        DEFAULT: 'var(--hv-transition-base)',
        slow: 'var(--hv-transition-slow)',
      },
    },
  },
  plugins: [],
}
