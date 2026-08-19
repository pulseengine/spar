package spar.osate.headless;

import java.io.File;
import java.io.FileInputStream;
import java.io.InputStream;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.stream.Collectors;
import java.util.stream.Stream;

import org.eclipse.core.resources.IProject;
import org.eclipse.core.resources.IProjectDescription;
import org.eclipse.core.resources.IResource;
import org.eclipse.core.resources.IWorkspace;
import org.eclipse.core.resources.ResourcesPlugin;
import org.eclipse.emf.common.util.URI;
import org.eclipse.emf.ecore.plugin.EcorePlugin;
import org.eclipse.emf.ecore.resource.Resource;
import org.eclipse.emf.ecore.util.EcoreUtil;
import org.eclipse.equinox.app.IApplication;
import org.eclipse.equinox.app.IApplicationContext;
import org.eclipse.xtext.resource.IResourceServiceProvider;
import org.eclipse.xtext.resource.XtextResource;
import org.eclipse.xtext.resource.XtextResourceSet;
import org.eclipse.xtext.util.CancelIndicator;
import org.eclipse.xtext.validation.CheckMode;
import org.eclipse.xtext.validation.IResourceValidator;
import org.eclipse.xtext.validation.Issue;
import org.osate.aadl2.AadlPackage;
import org.osate.aadl2.Classifier;
import org.osate.aadl2.SystemImplementation;
import org.osate.aadl2.instance.InstancePackage;
import org.osate.aadl2.instance.SystemInstance;
import org.osate.aadl2.instantiation.InstantiateModel;
import org.osate.aadl2.util.Aadl2ResourceFactoryImpl;
import org.osate.pluginsupport.PluginSupportUtil;
import org.osate.xtext.aadl2.Aadl2StandaloneSetup;
import org.osate.xtext.aadl2.errormodel.ErrorModelStandaloneSetup;

import com.google.inject.Injector;

/**
 * Headless OSATE application for spar conformance work. Two modes:
 *
 * instantiate: osate -nosplash -application spar.osate.headless.app -data /tmp/ws
 *              instantiate <projectDir> <Qualified::Impl.Name>...
 *   Loads all *.aadl under projectDir, instantiates each named system
 *   implementation, saves .aaxl2 to projectDir/instances/ (OSATE's canonical
 *   location, same file name scheme as the GUI's Instantiate command).
 *
 * validate:    ... validate <projectDir> [--isolate]
 *   Loads all *.aadl under projectDir (together, or each in its own resource
 *   set with --isolate), runs the full Xtext validator (CheckMode.ALL), and
 *   prints machine-readable verdicts:
 *     DIAG|<relpath>|<severity>|<line>|<message>
 *     VERDICT|<relpath>|ACCEPT or REJECT|<errorCount>
 */
public class InstantiateApp implements IApplication {

	private Injector injector;

	@Override
	public Object start(IApplicationContext context) throws Exception {
		context.applicationRunning();

		final String[] args = (String[]) context.getArguments().get("application.args");
		if (args == null || args.length < 2) {
			System.err.println("usage: instantiate <projectDir> <Qualified::Impl.Name>... | validate <projectDir> [--isolate]");
			return Integer.valueOf(2);
		}
		final String mode = args[0];
		final File projectDir = new File(args[1]).getAbsoluteFile();
		if (!projectDir.isDirectory()) {
			System.err.println("ERROR: not a directory: " + projectDir);
			return Integer.valueOf(2);
		}

		// --- standalone-ish setup inside the running platform (mirrors sireum Phantom) ---
		EcorePlugin.ExtensionProcessor.process(null);
		injector = new Aadl2StandaloneSetup().createInjectorAndDoEMFRegistration();
		ErrorModelStandaloneSetup.doSetup();
		Resource.Factory.Registry.INSTANCE.getExtensionToFactoryMap().put("aaxl2", new Aadl2ResourceFactoryImpl());
		InstancePackage.eINSTANCE.eClass();

		final String projName = projectDir.getName();
		EcorePlugin.getPlatformResourceMap().put(projName, URI.createFileURI(projectDir.getAbsolutePath() + "/"));

		// materialize a real workspace project at the directory so that
		// platform:/resource/<projName>/... resolves for org.eclipse.core.resources
		// (needed when saving .aaxl2 through PlatformResourceURIHandler)
		final IWorkspace ws = ResourcesPlugin.getWorkspace();
		final IProject project = ws.getRoot().getProject(projName);
		if (!project.exists()) {
			final IProjectDescription desc = ws.newProjectDescription(projName);
			desc.setLocation(new org.eclipse.core.runtime.Path(projectDir.getAbsolutePath()));
			project.create(desc, null);
		}
		project.open(null);
		project.refreshLocal(IResource.DEPTH_INFINITE, null);

		final List<Path> aadlFiles;
		try (Stream<Path> walk = Files.walk(projectDir.toPath())) {
			aadlFiles = walk.filter(p -> p.toString().endsWith(".aadl")).sorted().collect(Collectors.toList());
		}

		if ("validate".equals(mode)) {
			boolean isolate = args.length > 2 && "--isolate".equals(args[2]);
			return validate(projectDir, projName, aadlFiles, isolate);
		} else if ("instantiate".equals(mode)) {
			final List<String> implNames = new ArrayList<>();
			for (int i = 2; i < args.length; i++) {
				implNames.add(args[i]);
			}
			return instantiate(projectDir, projName, aadlFiles, implNames);
		} else {
			System.err.println("ERROR: unknown mode " + mode);
			return Integer.valueOf(2);
		}
	}

	private XtextResourceSet freshResourceSet() {
		XtextResourceSet rs = injector.getInstance(XtextResourceSet.class);
		// predeclared property sets + contributed packages (Base_Types, ARINC653, ...)
		for (URI uri : PluginSupportUtil.getContributedAadl()) {
			rs.getResource(uri, true);
		}
		return rs;
	}

	private Resource loadOne(XtextResourceSet rs, File projectDir, String projName, Path p) throws Exception {
		String rel = projectDir.toPath().relativize(p).toString().replace('\\', '/');
		URI uri = URI.createURI("platform:/resource/" + projName + "/" + rel);
		Resource res = rs.createResource(uri);
		try (InputStream in = new FileInputStream(p.toFile())) {
			res.load(in, Collections.EMPTY_MAP);
		}
		return res;
	}

	private Object validate(File projectDir, String projName, List<Path> aadlFiles, boolean isolate)
			throws Exception {
		if (isolate) {
			for (Path p : aadlFiles) {
				XtextResourceSet rs = freshResourceSet();
				Resource res = loadOne(rs, projectDir, projName, p);
				EcoreUtil.resolveAll(res);
				reportIssues(projectDir, p, res);
			}
		} else {
			XtextResourceSet rs = freshResourceSet();
			List<Resource> loaded = new ArrayList<>();
			List<Path> paths = new ArrayList<>();
			for (Path p : aadlFiles) {
				loaded.add(loadOne(rs, projectDir, projName, p));
				paths.add(p);
			}
			EcoreUtil.resolveAll(rs);
			for (int i = 0; i < loaded.size(); i++) {
				reportIssues(projectDir, paths.get(i), loaded.get(i));
			}
		}
		System.out.println("VALIDATE_DONE|" + aadlFiles.size());
		return IApplication.EXIT_OK;
	}

	private void reportIssues(File projectDir, Path p, Resource res) {
		String rel = projectDir.toPath().relativize(p).toString().replace('\\', '/');
		int errors = 0;
		if (res instanceof XtextResource) {
			IResourceServiceProvider rsp = ((XtextResource) res).getResourceServiceProvider();
			IResourceValidator validator = rsp.getResourceValidator();
			List<Issue> issues = validator.validate(res, CheckMode.ALL, CancelIndicator.NullImpl);
			for (Issue issue : issues) {
				String sev = String.valueOf(issue.getSeverity());
				if ("ERROR".equals(sev)) {
					errors++;
				}
				System.out.println("DIAG|" + rel + "|" + sev + "|" + issue.getLineNumber() + "|"
						+ String.valueOf(issue.getMessage()).replace('\n', ' ').replace('|', '/'));
			}
		} else {
			// non-xtext resource: fall back to EMF diagnostics
			for (Resource.Diagnostic d : res.getErrors()) {
				errors++;
				System.out.println("DIAG|" + rel + "|ERROR|" + d.getLine() + "|"
						+ String.valueOf(d.getMessage()).replace('\n', ' ').replace('|', '/'));
			}
		}
		System.out.println("VERDICT|" + rel + "|" + (errors == 0 ? "ACCEPT" : "REJECT") + "|" + errors);
	}

	private Object instantiate(File projectDir, String projName, List<Path> aadlFiles, List<String> implNames)
			throws Exception {
		XtextResourceSet rs = freshResourceSet();
		for (Path p : aadlFiles) {
			Resource res = loadOne(rs, projectDir, projName, p);
			System.out.println("loaded: " + res.getURI());
		}
		EcoreUtil.resolveAll(rs);

		int errCount = 0;
		for (Resource res : rs.getResources()) {
			for (Resource.Diagnostic d : res.getErrors()) {
				System.err.println("ERROR " + res.getURI().lastSegment() + ":" + d.getLine() + " " + d.getMessage());
				errCount++;
			}
		}
		if (errCount > 0) {
			System.err.println("ABORT: " + errCount + " parse/link errors");
			return Integer.valueOf(1);
		}

		int failures = 0;
		for (String qualified : implNames) {
			SystemImplementation impl = findImpl(rs, qualified);
			if (impl == null) {
				System.err.println("ERROR: system implementation not found: " + qualified);
				failures++;
				continue;
			}
			System.out.println("instantiating: " + qualified);
			SystemInstance si = InstantiateModel.instantiate(impl);
			if (si == null) {
				System.err.println("ERROR: instantiation returned null for " + qualified
						+ " (" + InstantiateModel.getErrorMessage() + ")");
				failures++;
				continue;
			}
			Resource ir = si.eResource();
			ir.save(null);
			System.out.println("saved: " + ir.getURI());
		}

		return failures == 0 ? IApplication.EXIT_OK : Integer.valueOf(1);
	}

	private SystemImplementation findImpl(XtextResourceSet rs, String qualified) {
		final int sep = qualified.lastIndexOf("::");
		final String pkgName = sep >= 0 ? qualified.substring(0, sep) : null;
		final String implName = sep >= 0 ? qualified.substring(sep + 2) : qualified;
		for (Resource res : rs.getResources()) {
			if (res.getContents().isEmpty() || !(res.getContents().get(0) instanceof AadlPackage)) {
				continue;
			}
			AadlPackage pkg = (AadlPackage) res.getContents().get(0);
			if (pkgName != null && !pkgName.equalsIgnoreCase(pkg.getName())) {
				continue;
			}
			if (pkg.getOwnedPublicSection() == null) {
				continue;
			}
			for (Classifier cl : pkg.getOwnedPublicSection().getOwnedClassifiers()) {
				if (cl instanceof SystemImplementation && implName.equalsIgnoreCase(cl.getName())) {
					return (SystemImplementation) cl;
				}
			}
		}
		return null;
	}

	@Override
	public void stop() {
	}
}
