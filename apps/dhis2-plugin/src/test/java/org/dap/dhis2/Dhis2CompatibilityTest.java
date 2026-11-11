// SPDX-License-Identifier: Apache-2.0
package org.dap.dhis2;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.*;

class Dhis2CompatibilityTest {

    @Test
    void supportsSupportedRange() {
        assertTrue(Dhis2Compatibility.supports("2.41"));
        assertTrue(Dhis2Compatibility.supports("2.41.2"));
        assertTrue(Dhis2Compatibility.supports("2.42"));
        assertTrue(Dhis2Compatibility.supports("2.43"));
    }

    @Test
    void rejectsPreviousMinor() {
        assertFalse(Dhis2Compatibility.supports("2.40"));
        assertFalse(Dhis2Compatibility.supports("2.39.4"));
        assertFalse(Dhis2Compatibility.supports("2.0.0"));
    }

    @Test
    void rejectsNullAndMalformed() {
        assertFalse(Dhis2Compatibility.supports(null));
        assertFalse(Dhis2Compatibility.supports("v2.41"));
        assertFalse(Dhis2Compatibility.supports("2"));
        assertFalse(Dhis2Compatibility.supports("abc"));
    }
}
