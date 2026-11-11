// SPDX-License-Identifier: Apache-2.0
package org.dap.dhis2;

/**
 * Compile-time compatibility declaration. Updated alongside
 * {@code apps/dhis2-plugin/pom.xml} when new DHIS2 minor releases are validated.
 */
public final class Dhis2Compatibility {

    /** The minimum DHIS2 version this plug-in binds against. */
    public static final String MIN_VERSION = "2.41";

    /** The highest DHIS2 version the compat matrix exercises today. */
    public static final String MAX_TESTED_VERSION = "2.43";

    private Dhis2Compatibility() {}

    /**
     * Returns true iff {@code major.minor} version falls inside the supported window.
     * DHIS2 version strings look like {@code "2.41"} or {@code "2.41.2"}. We compare
     * on {@code major.minor} only.
     */
    public static boolean supports(String dhis2Version) {
        if (dhis2Version == null) return false;
        String[] parts = dhis2Version.split("\\.");
        if (parts.length < 2) return false;
        try {
            int major = Integer.parseInt(parts[0]);
            int minor = Integer.parseInt(parts[1]);
            int minMajor = 2, minMinor = 41;
            int maxMajor = 2, maxMinor = 99; // forward-compatible until SPI changes
            if (major < minMajor) return false;
            if (major == minMajor && minor < minMinor) return false;
            if (major > maxMajor) return false;
            if (major == maxMajor && minor > maxMinor) return false;
            return true;
        } catch (NumberFormatException e) {
            return false;
        }
    }
}
