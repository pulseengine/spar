// SysML v2 Example: Comment Styles
// Tests various comment and documentation forms

// Line comment at top level

/* Block comment at top level */

package CommentStylesExample {
    doc "Package-level documentation"

    // Line comment before definition
    part def A {
        doc "Part A documentation"
        comment "This is an inline comment"

        /* Block comment inside body */
        attribute x : Real;
    }

    /* Multi-line
       block comment
       spanning several lines */
    part def B {
        // Comment between members
        attribute y : Real;
        // Another comment
        attribute z : Real;
    }

    part def C {
        doc "Documented part C"
        comment "Additional comment for C"
        part nested : A;
    }
}
