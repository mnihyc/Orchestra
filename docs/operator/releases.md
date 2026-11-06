# Publishing releases

Commit `.github/release-notes/<tag>.md` before pushing a version tag. The tag
starts both CI and release packaging. Packaging can finish while CI runs, but
the release is published only after the CI run for that tag and commit succeeds.
The publication job checks CI and the tag again before changing release assets.
Failed or cancelled CI stops publication. If CI does not finish within five
hours, the wait fails with a message; rerun CI and the release workflow after
the cause is resolved. A rerun can finish a draft left by an interrupted upload
after checking its existing assets. The newest stable release is marked Latest;
older releases and prereleases do not displace it.

To reuse native and frontend artifacts, dispatch the release workflow for the
same tag with the numeric `source_run_id` of an earlier completed release run.
The inputs are checked against the tag and commit and retained for 14 days.
CI must still be green. Cache warming does not publish anything.

Environment images are published separately. Configure required reviewers for
the `release-images` environment, then dispatch the release workflow with
`publish_images` enabled. The approved run verifies the release's asset hashes
and adds the image index to its manifest. Image publication cannot be combined
with repackaging.
