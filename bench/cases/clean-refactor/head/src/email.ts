const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/** True when `email`, ignoring surrounding whitespace, looks like local@domain.tld. */
export function isValidEmail(email: string): boolean {
  return EMAIL.test(email.trim());
}
