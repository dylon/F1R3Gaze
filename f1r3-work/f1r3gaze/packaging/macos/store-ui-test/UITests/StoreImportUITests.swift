import Foundation
import XCTest

final class StoreImportUITests: XCTestCase {
    func testExternalProfileImportThroughNativeFolderPanel() throws {
        continueAfterFailure = false
        let source = URL(fileURLWithPath: Fixture.sourcePath, isDirectory: true)
        let target = URL(fileURLWithPath: Fixture.targetPath, isDirectory: true)
        let sourceBefore = try snapshot(source)
        XCTAssertFalse(FileManager.default.fileExists(atPath: target.path))

        let app = XCUIApplication(url: URL(fileURLWithPath: Fixture.appPath, isDirectory: true))
        defer { app.terminate() }
        app.launch()

        let yes = app.dialogs.buttons["Yes"]
        XCTAssertTrue(yes.waitForExistence(timeout: 30), app.debugDescription)
        yes.click()

        let open = app.buttons["Open"].firstMatch
        XCTAssertTrue(open.waitForExistence(timeout: 30), app.debugDescription)
        app.typeKey("g", modifierFlags: [.command, .shift])
        let pathField = app.textFields.firstMatch
        XCTAssertTrue(pathField.waitForExistence(timeout: 10), app.debugDescription)
        pathField.click()
        pathField.typeKey("a", modifierFlags: .command)
        pathField.typeText(Fixture.sourcePath)
        pathField.typeKey(.return, modifierFlags: [])
        XCTAssertTrue(open.waitForExistence(timeout: 10), app.debugDescription)
        open.click()

        let marker = target.appendingPathComponent("data/layout.json")
        let deadline = Date().addingTimeInterval(30)
        while !FileManager.default.fileExists(atPath: marker.path) && Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.25))
        }
        XCTAssertTrue(FileManager.default.fileExists(atPath: marker.path), app.debugDescription)

        let sourceAfter = try snapshot(source)
        let targetAfter = try snapshot(target)
        XCTAssertEqual(sourceAfter, sourceBefore, "The import changed the unsandboxed source")
        XCTAssertEqual(targetAfter, sourceBefore, "The container does not contain a complete source copy")
    }

    private func snapshot(_ root: URL) throws -> [String: Data] {
        var files: [String: Data] = [:]
        for section in ["config", "data", "state"] {
            let directory = root.appendingPathComponent(section, isDirectory: true)
            let enumerator = FileManager.default.enumerator(
                at: directory,
                includingPropertiesForKeys: [.isRegularFileKey],
                options: [.skipsHiddenFiles]
            )
            while let file = enumerator?.nextObject() as? URL {
                let relative = String(file.path.dropFirst(root.path.count + 1))
                if relative == "state/runtime" || relative.hasPrefix("state/runtime/") {
                    enumerator?.skipDescendants()
                    continue
                }
                let values = try file.resourceValues(forKeys: [.isRegularFileKey])
                if values.isRegularFile == true {
                    files[relative] = try Data(contentsOf: file)
                }
            }
        }
        return files
    }
}
